use crate::{Json, Map, Result};
use minijinja::{Environment, UndefinedBehavior, Value};

pub fn render(template: &str, vars: &Map) -> Result<String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_filter("tojson", tojson);
    env.add_template("_inline_", template)?;
    let tmpl = env.get_template("_inline_")?;
    Ok(tmpl.render(Value::from_serialize(vars))?)
}

pub fn render_json(template: &str, vars: Json) -> Result<String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_filter("tojson", tojson);
    env.add_template("_inline_", template)?;
    Ok(env
        .get_template("_inline_")?
        .render(Value::from_serialize(&vars))?)
}

fn tojson(value: Value) -> std::result::Result<String, minijinja::Error> {
    serde_json::to_string(&value)
        .map(|text| {
            text.replace('&', "\\u0026")
                .replace('<', "\\u003c")
                .replace('>', "\\u003e")
                .replace('\'', "\\u0027")
        })
        .map_err(|e| minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string()))
}
