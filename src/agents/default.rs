use crate::{
    Json, Map, Result, VERSION,
    agents::compaction,
    config::{recursive_merge, with_defaults},
    content::{get_content_string, message_extra, message_role},
    environments::Environment,
    error::{ErrorKind, MiniError},
    models::Model,
    template::render,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

pub const DEFAULT_TYPE: &str = "minisweagent.agents.default.DefaultAgent";
pub const INTERACTIVE_TYPE: &str = "minisweagent.agents.interactive.InteractiveAgent";

pub fn agent_defaults(interactive: bool) -> Json {
    let mut defaults = serde_json::Map::new();
    defaults.insert(
        "system_template".into(),
        json!("You are a helpful assistant that can interact with a computer."),
    );
    defaults.insert(
        "instance_template".into(),
        json!("Please solve this issue: {{task}}"),
    );
    defaults.insert("step_limit".into(), json!(0));
    defaults.insert("cost_limit".into(), json!(3.0));
    defaults.insert("wall_time_limit_seconds".into(), json!(0));
    defaults.insert("max_consecutive_format_errors".into(), json!(3));
    defaults.insert("output_path".into(), Value::Null);
    defaults.insert("compact_at".into(), json!(0.0));
    defaults.insert("compact_template".into(), json!("Pause the task: do not call any tools and do not continue the work. Reply only with a summary, as plain text.\n{% if recent %}Summarize in detail the recent part of the conversation so far, from the message that starts with <boundary>{{boundary}}</boundary> to the end. Tool outputs from this part will be removed afterwards, so keep everything from them that is still needed: exact file paths, function names, commands, errors, results.{% else %}Summarize the earlier part of the conversation so far, from the start up to (not including) the message that starts with <boundary>{{boundary}}</boundary>.{% endif %}\nUse the sections Goal, Constraints and preferences, Done, In progress, Key decisions, Next steps and Critical context. Keep it under {{max_tokens}} tokens."));
    if interactive {
        defaults.insert("mode".into(), json!("confirm"));
        defaults.insert("whitelist_actions".into(), json!([]));
        defaults.insert("confirm_exit".into(), json!(true));
        defaults.insert("wait_for_user_hint".into(), json!("If you meant to reply to the user, write your message and run `echo WAIT_FOR_USER` as your only command."));
        defaults.insert(
            "show_observations".into(),
            json!(!crate::run::env_flag("MSWEA_QUIET", false)),
        );
        defaults.insert("observation_preview_lines".into(), json!(4));
    }
    Value::Object(defaults)
}

pub struct DefaultAgent {
    pub model: Box<dyn Model>,
    pub env: Box<dyn Environment>,
    pub config: Map,
    pub interactive: bool,
    pub messages: Vec<Json>,
    pub extra_template_vars: Map,
    pub cost: f64,
    pub n_calls: u64,
    pub n_consecutive_format_errors: u64,
    pub compacted_history: Vec<Json>,
    pub compact_requested: bool,
    start_time: Instant,
    pub last_footer: Option<(Vec<String>, usize)>,
}

impl DefaultAgent {
    pub fn new(
        model: Box<dyn Model>,
        env: Box<dyn Environment>,
        mut config: Map,
        interactive: bool,
    ) -> Self {
        config = with_defaults(config, agent_defaults(interactive));
        Self {
            model,
            env,
            config,
            interactive,
            messages: Vec::new(),
            extra_template_vars: Map::new(),
            cost: 0.0,
            n_calls: 0,
            n_consecutive_format_errors: 0,
            compacted_history: Vec::new(),
            compact_requested: false,
            start_time: Instant::now(),
            last_footer: None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        if self.interactive {
            INTERACTIVE_TYPE
        } else {
            DEFAULT_TYPE
        }
    }
    pub fn output_path(&self) -> Option<PathBuf> {
        self.config
            .get("output_path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }
    pub fn set_output_path(&mut self, path: Option<PathBuf>) {
        self.config.insert(
            "output_path".into(),
            path.map(|p| json!(p.to_string_lossy()))
                .unwrap_or(Value::Null),
        );
    }
    pub fn mode(&self) -> &str {
        self.config
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("confirm")
    }
    pub fn set_mode(&mut self, mode: &str) {
        self.config.insert("mode".into(), json!(mode));
    }
    pub fn confirm_exit(&self) -> bool {
        self.config
            .get("confirm_exit")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }
    pub fn set_confirm_exit(&mut self, value: bool) {
        self.config.insert("confirm_exit".into(), json!(value));
    }
    pub fn step_limit(&self) -> u64 {
        self.config
            .get("step_limit")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }
    pub fn cost_limit(&self) -> f64 {
        self.config
            .get("cost_limit")
            .and_then(Value::as_f64)
            .unwrap_or(3.0)
    }
    pub fn set_limits(&mut self, steps: u64, cost: f64) {
        self.config.insert("step_limit".into(), json!(steps));
        self.config.insert("cost_limit".into(), json!(cost));
    }

    pub fn get_template_vars(&self) -> Map {
        recursive_merge([
            Value::Object(self.config.clone()), Value::Object(self.env.get_template_vars()), Value::Object(self.model.get_template_vars()),
            json!({"n_model_calls":self.n_calls,"model_cost":self.cost,"elapsed_seconds":self.start_time.elapsed().as_secs()}),
            Value::Object(self.extra_template_vars.clone()),
        ]).as_object().cloned().unwrap_or_default()
    }

    pub fn render_template(&self, template: &str) -> Result<String> {
        render(template, &self.get_template_vars())
    }

    pub fn add_messages(&mut self, messages: Vec<Json>) -> Vec<Json> {
        if self.interactive {
            self.print_messages(&messages);
        }
        self.messages.extend(messages.iter().cloned());
        messages
    }

    pub fn run(&mut self, task: String, messages: Option<Vec<Json>>, extra: Map) -> Result<Json> {
        self.extra_template_vars.insert("task".into(), json!(task));
        self.extra_template_vars.extend(extra);
        self.messages = messages
            .unwrap_or_default()
            .into_iter()
            .filter(|m| message_role(m) != "exit")
            .collect();
        if !self.messages.is_empty() {
            if self.messages.last().is_some_and(|m| {
                m.pointer("/extra/actions")
                    .is_some_and(|a| a.as_array().is_some_and(|a| !a.is_empty()))
            }) {
                let vars = self.get_template_vars();
                let observations = self.model.format_observation_messages(
                    self.messages.last().unwrap(),
                    &[],
                    &vars,
                )?;
                self.add_messages(observations);
            }
            let message = self
                .model
                .format_message(json!({"role":"user","content":task}));
            self.add_messages(vec![message]);
        } else {
            let system = self.model.format_message(json!({"role":"system","content":self.render_template(self.config.get("system_template").and_then(Value::as_str).unwrap_or(""))?}));
            let instance = self.model.format_message(json!({"role":"user","content":self.render_template(self.config.get("instance_template").and_then(Value::as_str).unwrap_or(""))?}));
            self.add_messages(vec![system, instance]);
        }
        loop {
            let result = self.step();
            match result {
                Ok(_) => self.n_consecutive_format_errors = 0,
                Err(error) if error.kind == ErrorKind::Format => {
                    self.cost += error
                        .messages
                        .iter()
                        .map(|m| {
                            m.pointer("/extra/cost")
                                .and_then(Value::as_f64)
                                .unwrap_or(0.0)
                        })
                        .sum::<f64>();
                    let mut messages = error.messages;
                    let first = messages.first().cloned().unwrap_or_else(|| json!({}));
                    if first.pointer("/extra/interrupt_type").is_none()
                        && (messages.iter().any(|m| {
                            m.pointer("/extra/has_tool_calls").and_then(Value::as_bool)
                                == Some(true)
                        }) || get_content_string(&first).trim().is_empty())
                        && !messages.is_empty()
                    {
                        messages.remove(0);
                    }
                    self.n_consecutive_format_errors += 1;
                    let limit = self
                        .config
                        .get("max_consecutive_format_errors")
                        .and_then(Value::as_u64)
                        .unwrap_or(3);
                    if limit > 0 && limit <= self.n_consecutive_format_errors {
                        messages.push(json!({"role":"exit","content":"RepeatedFormatError","extra":{"exit_status":"RepeatedFormatError","submission":""}}));
                    }
                    self.add_messages(messages);
                }
                Err(error)
                    if matches!(
                        error.kind,
                        ErrorKind::Submitted
                            | ErrorKind::WaitForUser
                            | ErrorKind::Limits
                            | ErrorKind::Time
                            | ErrorKind::UserInterruption
                            | ErrorKind::UserReply
                            | ErrorKind::UserNewTask
                            | ErrorKind::UserRejection
                    ) =>
                {
                    self.add_messages(error.messages);
                }
                Err(error) => {
                    let text = error.to_string();
                    let name = error.type_name();
                    self.add_messages(vec![self.model.format_message(json!({"role":"exit","content":text,"extra":{"exit_status":name,"submission":"","exception_str":text,"traceback":text}}))]);
                    let _ = self.save(self.output_path(), Vec::new());
                    return Err(error);
                }
            }
            let _ = self.save(self.output_path(), Vec::new());
            if self
                .messages
                .last()
                .is_some_and(|m| message_role(m) == "exit")
            {
                break;
            }
        }
        Ok(Value::Object(message_extra(
            self.messages.last().unwrap_or(&json!({})),
        )))
    }

    pub fn step(&mut self) -> Result<Vec<Json>> {
        if self.interactive {
            println!("\n{}", "─".repeat(40));
        }
        let result = self
            .query()
            .and_then(|message| self.execute_actions(&message));
        if self.interactive {
            if let Err(error) = &result {
                if error.kind == ErrorKind::Format
                    && self.confirm_exit()
                    && !error.messages.iter().any(|m| {
                        m.pointer("/extra/has_tool_calls").and_then(Value::as_bool) == Some(true)
                    })
                {
                    let hint = self.model.format_message(json!({"role":"user","content":self.config.get("wait_for_user_hint").cloned().unwrap_or_else(||json!("")),"extra":{"interrupt_type":"FormatError"}}));
                    let mut messages = error.messages.clone();
                    messages.push(hint);
                    self.print_footer();
                    return Err(MiniError {
                        kind: error.kind,
                        messages,
                        source: None,
                    });
                }
            }
            self.print_footer();
        }
        result
    }

    pub fn query(&mut self) -> Result<Json> {
        if self.interactive && self.mode() == "human" {
            let command = self.prompt_and_handle_slash_commands("> ")?;
            if !matches!(command.as_str(), "/y" | "/c") {
                let message = json!({"role":"user","content":format!("User command: \n```bash\n{command}\n```"),"extra":{"actions":[{"command":command}]}});
                self.add_messages(vec![message.clone()]);
                return Ok(message);
            }
        }
        if (self.step_limit() > 0 && self.step_limit() <= self.n_calls)
            || (self.cost_limit() > 0.0 && self.cost_limit() <= self.cost)
        {
            if self.interactive && self.stdin_interactive() {
                println!(
                    "Limits exceeded. Limits: {} steps, ${:.2}. Current spend: {} steps, ${:.2}.",
                    self.step_limit(),
                    self.cost_limit(),
                    self.n_calls,
                    self.cost
                );
                let steps = self.prompt_line("New step limit: ")?;
                let cost = self.prompt_line("New cost limit: ")?;
                self.set_limits(
                    steps.trim().parse().unwrap_or(self.step_limit()),
                    cost.trim().parse().unwrap_or(self.cost_limit()),
                );
            } else {
                return Err(MiniError::exit("LimitsExceeded"));
            }
        }
        let wall = self
            .config
            .get("wall_time_limit_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if wall > 0 && wall <= self.start_time.elapsed().as_secs() {
            return Err(MiniError::exit("TimeExceeded"));
        }
        if self.should_compact() {
            self.compact()?;
        }
        self.n_calls += 1;
        let message = self.model.query(&self.messages)?;
        self.cost += message
            .pointer("/extra/cost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        self.add_messages(vec![message.clone()]);
        Ok(message)
    }

    pub fn should_compact(&self) -> bool {
        if !self.messages.is_empty() && self.compact_requested {
            return true;
        }
        let compact_at = self
            .config
            .get("compact_at")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if compact_at == 0.0 || self.messages.len() < 4 {
            return false;
        }
        self.model.get_context_window().is_some_and(|window| {
            window > 0
                && compaction::context_tokens(&self.messages) as f64 >= compact_at * window as f64
        })
    }

    pub fn compact(&mut self) -> Result<()> {
        self.compact_requested = false;
        if self.interactive {
            println!("Compacting the conversation...");
        }
        let window = self.model.get_context_window().unwrap_or(0);
        let scale = if window > 0 {
            (0.4 * window as f64 / 24_000.0).min(1.0)
        } else {
            1.0
        };
        let plan = compaction::plan(
            &self.messages,
            (5000.0 * scale) as usize,
            (5000.0 * scale) as usize,
            (10000.0 * scale) as usize,
            0.3,
        );
        let boundary: String = get_content_string(&self.messages[plan.recent_start])
            .chars()
            .take(150)
            .collect();
        let summary_tokens = (2000.0 * scale) as u64;
        let cached = window == 0
            || compaction::context_tokens(&self.messages) + 2 * summary_tokens as usize + 500
                < window as usize;
        let early = self.summarize(cached, false, &boundary, summary_tokens)?;
        let recent = self.summarize(cached, true, &boundary, summary_tokens)?;
        self.cost += early.1 + recent.1;
        let note = |content: String| {
            self.model.format_message(
                json!({"role":"user","content":content,"extra":{"interrupt_type":"Compaction"}}),
            )
        };
        let excerpt = |indices: &[usize]| {
            indices
                .iter()
                .map(|&i| compaction::strip_output(&self.messages[i]))
                .collect::<Vec<_>>()
        };
        let gap = |indices: &[usize]| {
            let mut result: Vec<_> = indices
                .iter()
                .map(|&i| self.messages[i].clone())
                .filter(compaction::is_human)
                .collect();
            if !indices.is_empty() {
                result.push(note(format!(
                    "[{} earlier messages were removed at compaction, see the summary below]",
                    indices.len()
                )));
            }
            result
        };
        let summary = note(format!(
            "The conversation was compacted to save context: tool outputs were removed and only excerpts and the user's messages remain.\n\n<summary>\n## Earlier\n{}\n\n## Recent, in detail\n{}\n</summary>",
            early.0, recent.0
        ));
        self.compacted_history.push(json!(self.messages));
        let mut messages = self.messages[..2].to_vec();
        messages.extend(excerpt(&plan.first));
        messages.extend(gap(&plan.gap1));
        messages.extend(excerpt(&plan.middle));
        messages.extend(gap(&plan.gap2));
        messages.push(summary);
        messages.extend(excerpt(&plan.last));
        self.messages = messages;
        let compact_at = self
            .config
            .get("compact_at")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if compact_at > 0.0
            && window > 0
            && compaction::context_tokens(&self.messages) as f64 >= compact_at * window as f64
        {
            return Err(crate::error::value_error(
                "Compaction could not bring the conversation below the compaction threshold",
            ));
        }
        if self.interactive {
            println!(
                "Compacted to about {} tokens.",
                fmt_tokens(compaction::estimate_tokens(&self.messages))
            );
        }
        Ok(())
    }

    fn summarize(
        &mut self,
        cached: bool,
        recent_flag: bool,
        boundary: &str,
        max_tokens: u64,
    ) -> Result<(String, f64)> {
        let template = self
            .config
            .get("compact_template")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let mut vars = self.get_template_vars();
        vars.insert("recent".into(), json!(recent_flag));
        vars.insert("boundary".into(), json!(boundary));
        vars.insert("max_tokens".into(), json!(max_tokens));
        let instruction = render(&template, &vars)?;
        let messages = if cached {
            let mut messages = self.messages.clone();
            messages.push(
                self.model
                    .format_message(json!({"role":"user","content":instruction})),
            );
            messages
        } else {
            let transcript = self.messages[1..]
                .iter()
                .map(|m| {
                    format!(
                        "[{}]: {}",
                        message_role(m),
                        get_content_string(&compaction::strip_output(m))
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            vec![self.messages[0].clone(), self.model.format_message(json!({"role":"user","content":format!("<conversation>\n{transcript}\n</conversation>\n\n{instruction}")}))]
        };
        let mut cost = 0.0;
        for _ in 0..2 {
            match self.model.query(&messages) {
                Ok(message) => {
                    cost += message
                        .pointer("/extra/cost")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                }
                Err(error) if error.kind == ErrorKind::Format => {
                    cost += error
                        .messages
                        .iter()
                        .map(|m| {
                            m.pointer("/extra/cost")
                                .and_then(Value::as_f64)
                                .unwrap_or(0.0)
                        })
                        .sum::<f64>();
                    if error
                        .messages
                        .first()
                        .is_some_and(|m| m.pointer("/extra/interrupt_type").is_none())
                    {
                        let text = get_content_string(&error.messages[0]).trim().to_owned();
                        if !text.is_empty() {
                            return Ok((text, cost));
                        }
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(crate::error::value_error(
            "Compaction failed: the model did not reply with a summary",
        ))
    }

    pub fn execute_actions(&mut self, message: &Json) -> Result<Vec<Json>> {
        let actions = message
            .pointer("/extra/actions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if self.interactive {
            self.ask_confirmation_or_interrupt(&actions)?;
        }
        let mut outputs = Vec::new();
        let result = (|| -> Result<()> {
            for action in &actions {
                let output = self.env.execute(action, "", None)?;
                if self.interactive
                    && self.confirm_exit()
                    && output.output.trim() == "WAIT_FOR_USER"
                {
                    return Err(MiniError::new(
                        ErrorKind::WaitForUser,
                        json!({"role":"exit","content":"","extra":{"exit_status":"Submitted","submission":""}}),
                    ));
                }
                outputs.push(output);
            }
            Ok(())
        })();
        let mut pending = result;
        if let Err(error) = pending.as_ref() {
            if !self.interactive {
                return Err(MiniError::new(
                    error.kind,
                    error.messages.first().cloned().unwrap_or_else(|| json!({})),
                ));
            }
            if matches!(error.kind, ErrorKind::Submitted | ErrorKind::WaitForUser) {
                if let Err(new_error) = self.check_for_new_task_or_submit(error) {
                    pending = Err(new_error);
                }
            }
        }
        let vars = self.get_template_vars();
        let observations = self
            .model
            .format_observation_messages(message, &outputs, &vars)?;
        let observations = self.add_messages(observations);
        match pending {
            Ok(()) => Ok(observations),
            Err(error) => Err(error),
        }
    }

    fn ask_confirmation_or_interrupt(&mut self, actions: &[Json]) -> Result<()> {
        if self.mode() != "confirm" {
            return Ok(());
        }
        let whitelist = self
            .config
            .get("whitelist_actions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let commands: Vec<_> = actions
            .iter()
            .filter_map(|a| a.get("command").and_then(Value::as_str))
            .collect();
        let needs = commands.iter().any(|command| {
            !whitelist.iter().filter_map(Value::as_str).any(|pattern| {
                regex::Regex::new(pattern)
                    .ok()
                    .and_then(|re| re.find(command))
                    .is_some()
            })
        });
        if !needs {
            return Ok(());
        }
        println!("Agent wants to run {} action(s):", commands.len());
        for command in &commands {
            println!("  {command}");
        }
        let answer = self.prompt_and_handle_slash_commands(
            "Execute actions? Enter to confirm, type a comment to reject, /h for commands\n> ",
        )?;
        match answer.trim() {
            "" | "/y" => Ok(()),
            "/u" => Err(MiniError::new(
                ErrorKind::UserRejection,
                json!({"role":"user","content":"Commands not executed. Switching to human mode","extra":{"interrupt_type":"UserRejection"}}),
            )),
            _ => Err(MiniError::new(
                ErrorKind::UserRejection,
                json!({"role":"user","content":format!("Commands not executed. The user rejected your commands with the following message: {answer}"),"extra":{"interrupt_type":"UserRejection"}}),
            )),
        }
    }

    fn check_for_new_task_or_submit(&mut self, error: &MiniError) -> Result<()> {
        if !self.confirm_exit() {
            return Err(MiniError::new(
                error.kind,
                error.messages.first().cloned().unwrap_or_else(|| json!({})),
            ));
        }
        loop {
            let prompt = if error.kind == ErrorKind::WaitForUser {
                "Your turn: reply, or Enter to quit (/h for commands)\n> "
            } else {
                "Agent wants to finish. Type new task or Enter to quit (/h for commands)\n> "
            };
            let answer = self
                .prompt_and_handle_slash_commands(prompt)?
                .trim()
                .to_owned();
            if answer == "/u" {
                return Err(MiniError::new(
                    ErrorKind::UserInterruption,
                    json!({"role":"user","content":"Switched to human mode.","extra":{"interrupt_type":"UserInterruption"}}),
                ));
            }
            if matches!(answer.as_str(), "/y" | "/c") {
                continue;
            }
            if !answer.is_empty() {
                let (kind, content) = if error.kind == ErrorKind::WaitForUser {
                    (ErrorKind::UserReply, format!("The user replied: {answer}"))
                } else {
                    (
                        ErrorKind::UserNewTask,
                        format!("The user added a new task: {answer}"),
                    )
                };
                return Err(MiniError::new(
                    kind,
                    json!({"role":"user","content":content,"extra":{"interrupt_type":if kind==ErrorKind::UserReply{"UserReply"}else{"UserNewTask"}}}),
                ));
            }
            return Err(MiniError::new(
                error.kind,
                error.messages.first().cloned().unwrap_or_else(|| json!({})),
            ));
        }
    }

    pub fn prompt_line(&self, prompt: &str) -> Result<String> {
        print!("{prompt}");
        let mut stdout = std::io::stdout();
        std::io::Write::flush(&mut stdout)?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Err(crate::error::value_error("EOF while reading input"));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }

    fn stdin_interactive(&self) -> bool {
        std::io::IsTerminal::is_terminal(&std::io::stdin())
    }

    pub fn prompt_and_handle_slash_commands(&mut self, prompt: &str) -> Result<String> {
        loop {
            let input = self.prompt_line(prompt)?;
            match input.as_str() {
                "/m" => return self.multiline_prompt(),
                "/h" => {
                    println!(
                        "Current mode: {}\n/y yolo  /c confirm  /u human  /m multiline  /e [level] thinking  /compact compact  /model [name] switch model",
                        self.mode()
                    );
                    continue;
                }
                "/compact" => {
                    self.compact_requested = self.messages.len() > 3;
                    println!(
                        "{}",
                        if self.compact_requested {
                            "Compacting before the next model call."
                        } else {
                            "Nothing to compact yet."
                        }
                    );
                    continue;
                }
                "/y" | "/c" | "/u" => {
                    let mode = match input.as_str() {
                        "/y" => "yolo",
                        "/c" => "confirm",
                        _ => "human",
                    };
                    if self.mode() == mode {
                        println!("Already in {mode} mode.");
                        continue;
                    }
                    self.set_mode(mode);
                    println!("Switched to {mode} mode.");
                    return Ok(input);
                }
                _ => {}
            }
            if input == "/e" || input.starts_with("/e ") {
                self.handle_thinking_command(input.trim_start_matches("/e").trim());
                continue;
            }
            if input == "/model" || input.starts_with("/model ") {
                self.handle_model_command(input.trim_start_matches("/model").trim());
                continue;
            }
            return Ok(input);
        }
    }

    fn multiline_prompt(&self) -> Result<String> {
        println!("Enter multiline input. Finish with a single '.' on its own line.");
        let mut lines = Vec::new();
        loop {
            let line = self.prompt_line("")?;
            if line == "." {
                break;
            }
            lines.push(line);
        }
        Ok(lines.join("\n"))
    }

    fn handle_thinking_command(&mut self, level: &str) {
        let levels = self.model.get_thinking_levels().unwrap_or_else(|| {
            vec![
                "high".into(),
                "medium".into(),
                "low".into(),
                "minimal".into(),
                "none".into(),
            ]
        });
        if levels.is_empty() {
            println!("This model accepts no reasoning level.");
            return;
        }
        let chosen = if level.is_empty() {
            let current = self
                .model
                .config()
                .get("model_kwargs")
                .and_then(|m| m.get("reasoning_effort"))
                .and_then(Value::as_str)
                .unwrap_or("unset");
            levels
                .iter()
                .position(|l| l == current)
                .map(|i| levels[(i + 1) % levels.len()].clone())
                .unwrap_or_else(|| levels[0].clone())
        } else {
            level.to_lowercase()
        };
        if !levels.contains(&chosen) {
            println!(
                "Unknown thinking level '{chosen}'. Choose one of: {}",
                levels.join(", ")
            );
            return;
        }
        if let Some(kwargs) = self
            .model
            .config_mut()
            .get_mut("model_kwargs")
            .and_then(Value::as_object_mut)
        {
            kwargs.insert("reasoning_effort".into(), json!(chosen));
        }
        println!("Thinking level: {chosen}");
    }

    fn handle_model_command(&mut self, argument: &str) {
        let profiles = self
            .model
            .config()
            .get("model_profiles")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if profiles.is_empty() {
            println!("No model profiles configured.");
            return;
        }
        let names: Vec<_> = profiles.keys().cloned().collect();
        let selected = if argument.is_empty() {
            for (i, name) in names.iter().enumerate() {
                println!("{}. {name}", i + 1);
            }
            let answer = self
                .prompt_line("Number or name (Enter to cancel): ")
                .unwrap_or_default();
            if answer.is_empty() {
                return;
            }
            answer
                .parse::<usize>()
                .ok()
                .filter(|i| *i > 0 && *i <= names.len())
                .map(|i| names[i - 1].clone())
                .unwrap_or(answer)
        } else {
            argument.to_owned()
        };
        let matched: Vec<_> = names
            .iter()
            .filter(|n| {
                n.to_lowercase() == selected.to_lowercase()
                    || (!selected.is_empty()
                        && n.to_lowercase().starts_with(&selected.to_lowercase()))
            })
            .cloned()
            .collect();
        let name = if matched.len() == 1 {
            matched[0].clone()
        } else {
            selected
        };
        match self.model.apply_profile(&name) {
            Ok(model) => println!("Model: {name} ({model})"),
            Err(error) => println!("Could not switch model: {error}"),
        }
    }

    pub fn preflight(&mut self) -> Result<()> {
        let profiles = self
            .model
            .config()
            .get("model_profiles")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if !profiles.is_empty() {
            for (i, name) in profiles.keys().enumerate() {
                println!("{}. {name}", i + 1);
            }
            let choice = self.prompt_line("Model (number/name, Enter to keep): ")?;
            if !choice.is_empty() {
                self.handle_model_command(&choice);
            }
        } else {
            println!(
                "Model: {}",
                self.model
                    .config()
                    .get("model_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
        }
        let levels = self.model.get_thinking_levels().unwrap_or_else(|| {
            vec![
                "high".into(),
                "medium".into(),
                "low".into(),
                "minimal".into(),
                "none".into(),
            ]
        });
        if levels.is_empty() {
            return Ok(());
        }
        println!("Thinking level: {}", levels.join(", "));
        let choice = self.prompt_line("Thinking (number/name, Enter to keep): ")?;
        if !choice.is_empty() {
            self.handle_thinking_command(&choice);
        }
        Ok(())
    }

    fn print_messages(&self, messages: &[Json]) {
        for message in messages {
            let role = message_role(message);
            let content = get_content_string(message);
            match role {
                "tool" => {
                    if self
                        .config
                        .get("show_observations")
                        .and_then(Value::as_bool)
                        .unwrap_or(true)
                    {
                        println!("\nTool:\n{content}");
                    } else {
                        self.print_observation(message);
                    }
                }
                "assistant" => {
                    println!("\nmini-swe-agent:");
                    if let Some(reasoning) = message
                        .get("reasoning_content")
                        .or_else(|| message.get("reasoning"))
                        .or_else(|| message.get("thinking"))
                        .and_then(Value::as_str)
                    {
                        println!("{reasoning}");
                    }
                    println!("{content}");
                }
                _ => println!("\n{}:\n{content}", role.to_uppercase()),
            }
        }
    }

    fn print_observation(&self, message: &Json) {
        let extra = message_extra(message);
        let code = extra.get("returncode").and_then(Value::as_i64);
        println!(
            "↳ {}",
            code.map(|c| if c == -1 {
                "not executed".into()
            } else {
                format!("exit {c}")
            })
            .unwrap_or_else(|| "not executed".into())
        );
        let output = extra
            .get("raw_output")
            .and_then(Value::as_str)
            .unwrap_or("");
        let lines: Vec<_> = output.lines().collect();
        let limit = self
            .config
            .get("observation_preview_lines")
            .and_then(Value::as_u64)
            .unwrap_or(4) as usize;
        if lines.len() <= 2 * limit {
            for line in lines {
                println!("  {line}");
            }
        } else {
            for line in &lines[..limit] {
                println!("  {line}");
            }
            println!("  … {} lines elided …", lines.len() - 2 * limit);
            for line in &lines[lines.len() - limit..] {
                println!("  {line}");
            }
        }
    }

    fn print_footer(&mut self) {
        let model = self
            .model
            .config()
            .get("model_name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let mut segments = vec![model, format!("step {}", self.n_calls), fmt_cost(self.cost)];
        if let Some(window) = self.model.get_context_window() {
            if let Some(usage) = self
                .messages
                .iter()
                .rev()
                .find_map(|m| m.pointer("/extra/usage"))
            {
                let tokens = usage
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + usage
                        .get("completion_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                segments.push(format!(
                    "ctx {}/{} {}%",
                    fmt_tokens(tokens as usize),
                    fmt_tokens(window as usize),
                    100 * tokens / window
                ));
            }
        }
        let signature = (segments.clone(), self.messages.len());
        if self.last_footer.as_ref() != Some(&signature) {
            println!("{}", segments.join("  │  "));
            self.last_footer = Some(signature);
        }
    }

    pub fn serialize(&self) -> Json {
        let empty = json!({});
        let last = self.messages.last().unwrap_or(&empty);
        let mut data = json!({
            "info": {
                "model_stats": {"instance_cost":self.cost,"api_calls":self.n_calls},
                "config": {"agent":self.config,"agent_type":self.type_name()},
                "mini_version":VERSION,
                "exit_status":last.pointer("/extra/exit_status").cloned().unwrap_or_else(||json!("")),
                "submission":last.pointer("/extra/submission").cloned().unwrap_or_else(||json!(""))
            },
            "messages":self.messages,
            "trajectory_format":"mini-swe-agent-1.1"
        });
        if !self.compacted_history.is_empty() {
            data.as_object_mut()
                .unwrap()
                .insert("compacted_history".into(), json!(self.compacted_history));
        }
        recursive_merge([data, self.model.serialize(), self.env.serialize()])
    }

    pub fn save(&self, path: Option<PathBuf>, extra: Vec<Json>) -> Result<Json> {
        let mut data = self.serialize();
        for value in extra {
            data = recursive_merge([data, value]);
        }
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp = path.with_file_name(format!(
                "{}.tmp",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            fs::write(&tmp, serde_json::to_string_pretty(&data)?)?;
            fs::rename(&tmp, &path)?;
        }
        Ok(data)
    }
}

pub fn fmt_cost(cost: f64) -> String {
    if cost == 0.0 {
        "$0.00".into()
    } else if cost < 0.01 {
        format!("${cost:.6}")
    } else if cost < 1.0 {
        format!("${cost:.4}")
    } else {
        format!("${cost:.2}")
    }
}
pub fn fmt_tokens(count: usize) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1000.0)
    } else {
        count.to_string()
    }
}

pub fn get_agent(
    model: Box<dyn Model>,
    env: Box<dyn Environment>,
    mut config: Map,
    default_type: &str,
) -> Result<DefaultAgent> {
    let spec = config
        .remove("agent_class")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| default_type.to_owned());
    let class = spec.rsplit('.').next().unwrap_or("");
    let interactive = match class {
        "InteractiveAgent" | "interactive" => true,
        "DefaultAgent" | "default" => false,
        _ => {
            return Err(crate::error::value_error(format!(
                "Unknown agent type: {spec}"
            )));
        }
    };
    Ok(DefaultAgent::new(model, env, config, interactive))
}

#[allow(dead_code)]
fn _types(
    _: Option<HashMap<String, String>>,
    _: Option<SystemTime>,
    _: Option<MiniError>,
    _: Option<&Path>,
) {
}
