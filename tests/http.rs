use mini_swe_agent::{
    Map, Result,
    models::{Model, NativeModel, Provider, Style},
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn model(uri: String, style: Style) -> NativeModel {
    NativeModel::new(
        Provider::LiteLLM,
        style,
        Map::from_iter([
            ("model_name".into(), json!("openai/test")),
            (
                "model_kwargs".into(),
                json!({"api_base":uri,"drop_params":true}),
            ),
            (
                "pricing".into(),
                json!({"input":1.0,"output":2.0,"cache_read":0.1,"cache_write":1.5}),
            ),
        ]),
        "test.Model",
    )
    .unwrap()
}

#[tokio::test]
async fn chat_toolcall_request_and_cost() -> Result<()> {
    let server = MockServer::start().await;
    let body = json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"echo hi\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"cache_read_input_tokens":10}});
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;
    let messages = vec![json!({"role":"user","content":"hi","extra":{"secret":true}})];
    let uri = server.uri();
    let response =
        tokio::task::spawn_blocking(move || model(uri, Style::Toolcall).query(&messages))
            .await
            .unwrap()?;
    assert_eq!(response["extra"]["actions"][0]["command"], json!("echo hi"));
    let actual = response["extra"]["cost"].as_f64().unwrap();
    assert!((actual - 0.000131).abs() < 1e-12, "actual cost: {actual}");
    assert_eq!(response["extra"]["usage"]["cached_tokens"], json!(10));
    let requests = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&requests[0].body)?;
    assert!(sent["messages"][0].get("extra").is_none());
    assert!(sent.get("drop_params").is_none());
    assert_eq!(sent["tools"][0]["function"]["name"], json!("bash"));
    Ok(())
}

#[tokio::test]
async fn format_error_persists_response_and_cost() -> Result<()> {
    let server = MockServer::start().await;
    let body = json!({"choices":[{"message":{"role":"assistant","content":"I cannot call a tool"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}});
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;
    let messages = vec![json!({"role":"user","content":"hi"})];
    let uri = server.uri();
    let error = tokio::task::spawn_blocking(move || model(uri, Style::Toolcall).query(&messages))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, mini_swe_agent::error::ErrorKind::Format);
    assert_eq!(error.messages[0]["content"], json!("I cannot call a tool"));
    assert!(error.messages[1]["extra"]["response"].is_object());
    assert!(error.messages[1]["extra"]["cost"].as_f64().unwrap() > 0.0);
    assert_eq!(
        error.messages[1]["extra"]["interrupt_type"],
        json!("FormatError")
    );
    Ok(())
}

#[tokio::test]
async fn responses_api_flattens_history_and_observations() -> Result<()> {
    let server = MockServer::start().await;
    let body = json!({"object":"response","status":"completed","output":[{"type":"function_call","call_id":"r1","name":"bash","arguments":"{\"command\":\"pwd\"}"}],"usage":{"input_tokens":10,"output_tokens":5}});
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;
    let history = vec![
        json!({"object":"response","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"earlier"}]}],"extra":{"x":1}}),
        json!({"role":"user","content":"next","extra":{"y":2}}),
    ];
    let uri = server.uri();
    let response = tokio::task::spawn_blocking(move || model(uri, Style::Response).query(&history))
        .await
        .unwrap()?;
    assert_eq!(response["extra"]["actions"][0]["tool_call_id"], json!("r1"));
    let requests = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(sent["input"].as_array().unwrap().len(), 2);
    assert!(sent["input"][0].get("extra").is_none());
    assert_eq!(sent["tools"][0]["type"], json!("function"));
    assert!(sent["tools"][0].get("function").is_none());
    Ok(())
}
