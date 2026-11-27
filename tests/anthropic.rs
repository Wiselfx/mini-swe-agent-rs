use mini_swe_agent::{
    Map, Result,
    models::{Model, NativeModel, Provider, Style},
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn anthropic_native_messages_and_cache_control() -> Result<()> {
    let server = MockServer::start().await;
    let body = json!({"content":[{"type":"thinking","thinking":"considering"},{"type":"tool_use","id":"tool_1","name":"bash","input":{"command":"echo hi"}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":2}});
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&body))
        .mount(&server)
        .await;
    let old_base = std::env::var_os("ANTHROPIC_BASE_URL");
    let old_key = std::env::var_os("ANTHROPIC_API_KEY");
    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", format!("{}/v1", server.uri()));
        std::env::set_var("ANTHROPIC_API_KEY", "secret");
    }
    let messages = vec![json!({"role":"user","content":"hi"})];
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut model = NativeModel::new(
            Provider::LiteLLM,
            Style::Toolcall,
            Map::from_iter([
                ("model_name".into(), json!("anthropic/test")),
                ("set_cache_control".into(), json!("default_end")),
                (
                    "pricing".into(),
                    json!({"input":1.0,"output":2.0,"cache_read":0.1}),
                ),
            ]),
            "test.Anthropic",
        )?;
        model.query(&messages)
    })
    .await
    .unwrap();
    unsafe {
        if let Some(value) = old_base {
            std::env::set_var("ANTHROPIC_BASE_URL", value);
        } else {
            std::env::remove_var("ANTHROPIC_BASE_URL");
        }
        if let Some(value) = old_key {
            std::env::set_var("ANTHROPIC_API_KEY", value);
        } else {
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
    }
    let response = result?;
    assert_eq!(response["reasoning_content"], json!("considering"));
    assert_eq!(response["extra"]["actions"][0]["command"], json!("echo hi"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers.get("x-api-key").unwrap(), "secret");
    let sent: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(
        sent["messages"][0]["content"][0]["cache_control"]["type"],
        json!("ephemeral")
    );
    assert_eq!(sent["tools"][0]["name"], json!("bash"));
    Ok(())
}
