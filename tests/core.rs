use mini_swe_agent::{
    Json, Map,
    agents::{DefaultAgent, compaction},
    config::{get_config_from_spec, key_value_spec_to_nested_dict, recursive_merge},
    content::get_content_string,
    environments::{Environment, LocalEnvironment},
    error::{ErrorKind, MiniError},
    models::{
        DeterministicModel, DeterministicStyle, Style,
        actions::{
            expand_multimodal_content, format_toolcall_observation_messages, parse_regex_actions,
            parse_toolcall_actions, parse_toolcall_actions_response, prepare_messages,
            set_cache_control,
        },
    },
    template::render,
};
use serde_json::{Value, json};

fn deterministic(outputs: Vec<Json>) -> DeterministicModel {
    DeterministicModel::new(
        DeterministicStyle::Text,
        Map::from_iter([
            ("outputs".into(), json!(outputs)),
            ("cost_per_call".into(), json!(0.0)),
        ]),
    )
}

fn output(content: &str, command: &str) -> Json {
    json!({"role":"assistant","content":content,"extra":{"actions":[{"command":command}],"cost":0.0,"timestamp":0.0}})
}

#[test]
fn config_specs_merge_like_python() {
    let nested = key_value_spec_to_nested_dict("model.model_kwargs.temperature=0.5").unwrap();
    assert_eq!(nested["model"]["model_kwargs"]["temperature"], json!(0.5));
    let merged = recursive_merge([
        json!({"a":{"x":1,"y":2},"b":1}),
        json!({"a":{"y":3},"b":{"nested":true}}),
    ]);
    assert_eq!(merged, json!({"a":{"x":1,"y":3},"b":{"nested":true}}));
    assert_eq!(
        get_config_from_spec("mini.yaml").unwrap()["agent"]["mode"],
        json!("confirm")
    );
}

#[test]
fn strict_templates_and_builtin_observation() {
    assert!(render("{{ missing }}", &Map::new()).is_err());
    let config = get_config_from_spec("mini.yaml").unwrap();
    let template = config["model"]["observation_template"].as_str().unwrap();
    let mut vars = Map::new();
    vars.insert(
        "output".into(),
        json!({"returncode":0,"output":"hello","exception_info":""}),
    );
    assert_eq!(
        render(template, &vars).unwrap(),
        "{\n  \"returncode\": 0,\n  \"output\": \"hello\"\n}"
    );
    let mut vars = Map::new();
    vars.insert(
        "output".into(),
        json!({"returncode":1,"output":"x".repeat(10001),"exception_info":"boom"}),
    );
    let rendered = render(template, &vars).unwrap();
    assert!(rendered.contains("output_head"));
    assert!(rendered.contains("elided_chars"));
    assert!(rendered.contains("exception_info"));
}

#[test]
fn local_environment_executes_and_detects_submission() {
    let mut env = LocalEnvironment::new(Map::from_iter([
        ("env".into(), json!({"RUST_PORT_TEST":"yes"})),
        ("timeout".into(), json!(5)),
    ]));
    let result = env
        .execute(
            &json!({"command":"printf out; printf err >&2; printf \"$RUST_PORT_TEST\""}),
            "",
            None,
        )
        .unwrap();
    assert_eq!(result.returncode, 0);
    assert_eq!(result.output, "outerryes");
    let result = env.execute(&json!({"command":"exit 7"}), "", None).unwrap();
    assert_eq!(result.returncode, 7);
    let error = env
        .execute(
            &json!({"command":"echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT; printf patch"}),
            "",
            None,
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Submitted);
    assert_eq!(error.messages[0]["extra"]["submission"], json!("patch"));
    let timeout = env
        .execute(&json!({"command":"sleep 2"}), "", Some(1))
        .unwrap();
    assert_eq!(timeout.returncode, -1);
    assert_eq!(
        timeout.extra.as_ref().unwrap()["exception_type"],
        json!("TimeoutExpired")
    );
}

#[test]
fn toolcall_and_text_action_parsing() {
    let calls = json!([{"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]).as_array().unwrap().clone();
    let actions = parse_toolcall_actions(&calls, "{{ error }}", Some(&json!("stop"))).unwrap();
    assert_eq!(
        json!(actions),
        json!([{"command":"ls","tool_call_id":"call_1"}])
    );
    let error = parse_toolcall_actions(&[], "{{ error }}", None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Format);
    assert!(
        error.messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("No tool calls")
    );
    let actions = parse_regex_actions(
        "thought\n```mswea_bash_command\n echo hi \n```",
        r"```mswea_bash_command\s*\n(.*?)\n```",
        "{{error}}",
        None,
    )
    .unwrap();
    assert_eq!(json!(actions), json!([{"command":"echo hi"}]));
    let error = parse_regex_actions(
        "none",
        r"```mswea_bash_command\s*\n(.*?)\n```",
        "{{error}}",
        None,
    )
    .unwrap_err();
    assert_eq!(error.messages[0]["extra"]["n_actions"], json!(0));
    let response_calls = json!([{"type":"function_call","call_id":"r1","name":"bash","arguments":"{\"command\":\"pwd\"}"}]).as_array().unwrap().clone();
    assert_eq!(
        json!(parse_toolcall_actions_response(&response_calls, "{{error}}", None).unwrap()),
        json!([{"command":"pwd","tool_call_id":"r1"}])
    );
}

#[test]
fn observations_preserve_tool_ids_and_padding() {
    let actions =
        json!([{"command":"one","tool_call_id":"a"},{"command":"two","tool_call_id":"b"}])
            .as_array()
            .unwrap()
            .clone();
    let outputs = vec![mini_swe_agent::environments::CommandOutput::success(
        "ok".into(),
        0,
    )];
    let messages = format_toolcall_observation_messages(
        &actions,
        &outputs,
        "{{output.returncode}}:{{output.output}}",
        &Map::new(),
        "",
    )
    .unwrap();
    assert_eq!(messages[0]["role"], json!("tool"));
    assert_eq!(messages[0]["tool_call_id"], json!("a"));
    assert_eq!(messages[1]["content"], json!("-1:"));
    assert_eq!(
        messages[1]["extra"]["exception_info"],
        json!("action was not executed")
    );
    let response = mini_swe_agent::models::actions::format_toolcall_observation_messages_impl(
        &actions,
        &outputs,
        "{{output.output}}",
        &Map::new(),
        "",
        true,
    )
    .unwrap();
    assert_eq!(response[0]["type"], json!("function_call_output"));
    assert_eq!(response[0]["call_id"], json!("a"));
}

#[test]
fn multimodal_and_cache_control_are_structural() {
    let expanded = expand_multimodal_content(json!("before<MSWEA_MULTIMODAL_CONTENT><CONTENT_TYPE>image_url</CONTENT_TYPE>https://img</MSWEA_MULTIMODAL_CONTENT>after"), r"(?s)<MSWEA_MULTIMODAL_CONTENT><CONTENT_TYPE>(.+?)</CONTENT_TYPE>(.+?)</MSWEA_MULTIMODAL_CONTENT>").unwrap();
    assert_eq!(expanded[0]["text"], json!("before"));
    assert_eq!(expanded[1]["image_url"]["url"], json!("https://img"));
    assert_eq!(expanded[2]["text"], json!("after"));
    let plain = expand_multimodal_content(json!("plain"), r"<x>(.*?)</x>").unwrap();
    assert_eq!(plain, json!([{"type":"text","text":"plain"}]));
    let messages = vec![
        json!({"role":"system","content":"sys"}),
        json!({"role":"user","content":"task"}),
        json!({"role":"assistant","content":"reply"}),
        json!({"role":"user","content":"obs"}),
    ];
    let cached = set_cache_control(messages.clone(), Some("default_end")).unwrap();
    assert!(cached[1]["content"][0]["cache_control"].is_object());
    assert!(cached[3]["content"][0]["cache_control"].is_object());
    assert!(cached[0]["content"][0].get("cache_control").is_none());
    let prepared = prepare_messages(
        &[json!({"role":"user","content":"x","extra":{"secret":true}})],
        None,
    )
    .unwrap();
    assert!(prepared[0].get("extra").is_none());
}

#[test]
fn content_display_supports_all_wire_formats() {
    assert_eq!(
        get_content_string(&json!({"role":"assistant","content":"hello"})),
        "hello"
    );
    assert_eq!(
        get_content_string(
            &json!({"tool_calls":[{"function":{"arguments":"{\"command\":\"ls\"}"}}]})
        ),
        "```\nls\n```"
    );
    assert_eq!(
        get_content_string(
            &json!({"content":[{"type":"tool_result","content":"{\"returncode\":0,\"output\":\"ok\"}"}]})
        ),
        "<returncode>\n0\n<output>\nok"
    );
    assert_eq!(
        get_content_string(
            &json!({"output":[{"type":"function_call","arguments":"{\"command\":\"pwd\"}"}]})
        ),
        "```\npwd\n```"
    );
}

#[test]
fn default_agent_runs_to_submission_and_saves_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.traj.json");
    let model = deterministic(vec![
        output("inspect", "echo hi"),
        output(
            "finish",
            "echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT; printf patch",
        ),
    ]);
    let config = Map::from_iter([
        ("system_template".into(), json!("S:{{task}}")),
        ("instance_template".into(), json!("I:{{task}}")),
        ("cost_limit".into(), json!(0.0)),
        ("output_path".into(), json!(path.to_string_lossy())),
    ]);
    let mut agent = DefaultAgent::new(
        Box::new(model),
        Box::new(LocalEnvironment::new(Map::new())),
        config,
        false,
    );
    let info = agent.run("fix it".into(), None, Map::new()).unwrap();
    assert_eq!(info["exit_status"], json!("Submitted"));
    assert_eq!(info["submission"], json!("patch"));
    assert_eq!(agent.n_calls, 2);
    let saved: Json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["trajectory_format"], json!("mini-swe-agent-1.1"));
    assert_eq!(saved["messages"][0]["content"], json!("S:fix it"));
    assert_eq!(saved["messages"][1]["content"], json!("I:fix it"));
    assert_eq!(saved["info"]["model_stats"]["api_calls"], json!(2));
    assert!(
        saved["info"]["config"]["environment_type"]
            .as_str()
            .unwrap()
            .ends_with("LocalEnvironment")
    );
}

#[test]
fn limits_end_cleanly_and_resume_fills_missing_results() {
    let model = deterministic(vec![output("unused", "echo no")]);
    let config = Map::from_iter([
        ("system_template".into(), json!("s")),
        ("instance_template".into(), json!("{{task}}")),
        ("step_limit".into(), json!(1)),
        ("cost_limit".into(), json!(0.0)),
    ]);
    let mut agent = DefaultAgent::new(
        Box::new(model),
        Box::new(LocalEnvironment::new(Map::new())),
        config,
        false,
    );
    let info = agent.run("task".into(), None, Map::new()).unwrap();
    assert_eq!(info["exit_status"], json!("LimitsExceeded"));

    let messages = vec![
        json!({"role":"system","content":"s"}),
        json!({"role":"user","content":"old"}),
        json!({"role":"assistant","content":"pending","extra":{"actions":[{"command":"echo pending","tool_call_id":"x"}]}}),
    ];
    let model = DeterministicModel::new(
        DeterministicStyle::Toolcall,
        Map::from_iter([
            (
                "outputs".into(),
                json!([output("done", "echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT")]),
            ),
            ("cost_per_call".into(), json!(0.0)),
        ]),
    );
    let mut agent = DefaultAgent::new(
        Box::new(model),
        Box::new(LocalEnvironment::new(Map::new())),
        Map::from_iter([
            ("system_template".into(), json!("s")),
            ("instance_template".into(), json!("{{task}}")),
            ("cost_limit".into(), json!(0.0)),
        ]),
        false,
    );
    agent.run("new".into(), Some(messages), Map::new()).unwrap();
    assert_eq!(agent.messages[3]["role"], json!("tool"));
    assert_eq!(agent.messages[4]["role"], json!("user"));
    assert_eq!(agent.messages[4]["content"], json!("new"));
}

#[test]
fn compaction_plan_keeps_units_and_removes_outputs() {
    let messages = vec![
        json!({"role":"system","content":"sys"}),
        json!({"role":"user","content":"task"}),
        json!({"role":"assistant","content":"a","extra":{"actions":[{"command":"one"}]}}),
        json!({"role":"tool","content":"result","tool_call_id":"1","extra":{"returncode":0,"raw_output":"result"}}),
        json!({"role":"assistant","content":"b","extra":{"actions":[{"command":"two"}]}}),
        json!({"role":"tool","content":"result2","tool_call_id":"2","extra":{"returncode":0,"raw_output":"result2"}}),
    ];
    let plan = compaction::plan(&messages, 1, 100, 1, 0.3);
    assert!(
        plan.first
            .iter()
            .chain(&plan.middle)
            .chain(&plan.last)
            .any(|&i| i == 2)
    );
    let stripped = compaction::strip_output(&messages[3]);
    assert_eq!(stripped["content"], json!(compaction::OUTPUT_REMOVED));
    assert!(stripped["extra"].get("raw_output").is_none());
}

#[test]
fn format_error_kind_is_stable() {
    let error = MiniError::new(ErrorKind::Format, json!({"role":"user","content":"bad"}));
    assert_eq!(error.type_name(), "FormatError");
    let _ = Style::Toolcall;
    let _: Option<Value> = None;
}
