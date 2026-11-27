use mini_swe_agent::{Map, Result, environments::get_environment};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn contree_native_http_flow() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/import"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"uuid":"import-op"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/operations/import-op"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"SUCCESS","result_image_uuid":"image-1"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/instances"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"uuid":"run-op"})))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/v1/operations/run-op")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"SUCCESS","result_image_uuid":"image-2","metadata":{"result":{"stdout":{"encoding":"ascii","value":"hello"},"stderr":{"encoding":"ascii","value":"err"},"state":{"exit_code":3}}}}))).mount(&server).await;
    let uri = server.uri();
    let output = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut env = get_environment(
            Map::from_iter([
                ("environment_class".into(), json!("contree")),
                ("contree_config".into(), json!({"url":uri,"token":"secret"})),
                ("image".into(), json!("alpine")),
                ("cwd_auto_create".into(), json!(false)),
            ]),
            "contree",
        )?;
        env.execute(&json!({"command":"echo hello"}), "", None)
    })
    .await
    .unwrap()?;
    assert_eq!(output.output, "helloerr");
    assert_eq!(output.returncode, 3);
    let requests = server.received_requests().await.unwrap();
    let import: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(
        import["registry"]["url"],
        json!("docker://docker.io/library/alpine:latest")
    );
    assert_eq!(
        requests[0].headers.get("Authorization").unwrap(),
        "Bearer secret"
    );
    Ok(())
}

#[tokio::test]
async fn swerex_remote_runtime_flow() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/is_alive"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"is_alive":true})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/execute"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"stdout":"remote-ok","exit_code":0})),
        )
        .mount(&server)
        .await;
    let uri = server.uri();
    let output = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut env = get_environment(
            Map::from_iter([
                ("environment_class".into(), json!("swerex_modal")),
                ("runtime_url".into(), json!(uri)),
                ("auth_token".into(), json!("token")),
                ("image".into(), json!("alpine")),
            ]),
            "swerex_modal",
        )?;
        env.execute(&json!({"command":"echo remote-ok"}), "", None)
    })
    .await
    .unwrap()?;
    assert_eq!(output.output, "remote-ok");
    assert_eq!(output.returncode, 0);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[1].headers.get("X-API-Key").unwrap(), "token");
    let body: Value = serde_json::from_slice(&requests[1].body)?;
    assert_eq!(body["command"], json!("echo remote-ok"));
    assert_eq!(body["merge_output_streams"], json!(true));
    Ok(())
}
