use mini_swe_agent::{
    Map, Result, agents::DefaultAgent, environments::LocalEnvironment, models::get_model,
};
use serde_json::json;

fn main() -> Result<()> {
    let model = get_model(Map::from_iter([(
        "model_name".into(),
        json!("openai/gpt-5-mini"),
    )]))?;
    let mut agent = DefaultAgent::new(
        model,
        Box::new(LocalEnvironment::new(Map::new())),
        Map::from_iter([
            (
                "system_template".into(),
                json!("You are a helpful assistant."),
            ),
            ("instance_template".into(), json!("{{task}}")),
        ]),
        false,
    );
    agent.run("Write a sudoku game".into(), None, Map::new())?;
    Ok(())
}
