use clap::Parser;
use mini_swe_agent::{Json, Result, content::get_content_string, run::init};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
#[command(
    name = "inspector",
    version,
    about = "Browse agent conversation trajectories."
)]
struct Args {
    #[arg(default_value = ".")]
    path: String,
    #[arg(long = "no-reasoning")]
    no_reasoning: bool,
}

struct Inspector {
    files: Vec<PathBuf>,
    trajectory: usize,
    step: usize,
    messages: Vec<Json>,
    steps: Vec<Vec<Json>>,
    scroll: u16,
    reasoning: bool,
}

fn main() {
    init();
    if let Err(error) = execute(Args::parse()) {
        mini_swe_agent::run::report_error(&error);
        std::process::exit(1);
    }
}

fn execute(args: Args) -> Result<()> {
    let path = Path::new(&args.path);
    let files = if path.is_file() {
        vec![path.to_path_buf()]
    } else if path.is_dir() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        walk(&p, out);
                    } else if p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().ends_with(".traj.json"))
                    {
                        out.push(p);
                    }
                }
            }
        }
        let mut files = Vec::new();
        walk(path, &mut files);
        files.sort();
        files
    } else {
        return Err(mini_swe_agent::error::value_error(format!(
            "Path '{}' does not exist",
            path.display()
        )));
    };
    if files.is_empty() {
        return Err(mini_swe_agent::error::value_error(format!(
            "No trajectory files found in '{}'",
            path.display()
        )));
    }
    let mut inspector = Inspector {
        files,
        trajectory: 0,
        step: 0,
        messages: Vec::new(),
        steps: Vec::new(),
        scroll: 0,
        reasoning: !args.no_reasoning,
    };
    inspector.load()?;
    crossterm::terminal::enable_raw_mode()?;
    let mut terminal = ratatui::init();
    let result = loop {
        terminal.draw(|f| inspector.draw(f))?;
        if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
            if key.kind != crossterm::event::KeyEventKind::Press {
                continue;
            }
            match key.code {
                crossterm::event::KeyCode::Char('q') => break Ok(()),
                crossterm::event::KeyCode::Right | crossterm::event::KeyCode::Char('l') => {
                    inspector.next_step()
                }
                crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Char('h') => {
                    inspector.prev_step()
                }
                crossterm::event::KeyCode::Char('0') => {
                    inspector.step = 0;
                    inspector.scroll = 0;
                }
                crossterm::event::KeyCode::Char('$') => {
                    inspector.step = inspector.steps.len().saturating_sub(1);
                    inspector.scroll = 0;
                }
                crossterm::event::KeyCode::Char('L') => inspector.next_trajectory(),
                crossterm::event::KeyCode::Char('H') => inspector.prev_trajectory(),
                crossterm::event::KeyCode::Char('j') | crossterm::event::KeyCode::Down => {
                    inspector.scroll = inspector.scroll.saturating_add(15)
                }
                crossterm::event::KeyCode::Char('k') | crossterm::event::KeyCode::Up => {
                    inspector.scroll = inspector.scroll.saturating_sub(15)
                }
                crossterm::event::KeyCode::Char('r') => {
                    inspector.reasoning = !inspector.reasoning;
                    inspector.scroll = 0;
                }
                crossterm::event::KeyCode::Char('R') => {
                    inspector.load()?;
                }
                crossterm::event::KeyCode::Char('e') | crossterm::event::KeyCode::Char('E') => {
                    let all = key.code == crossterm::event::KeyCode::Char('E');
                    crossterm::terminal::disable_raw_mode()?;
                    terminal.show_cursor()?;
                    let temp = tempfile::Builder::new().suffix(".json").tempfile()?;
                    let data = if all {
                        json_files(&inspector.files[inspector.trajectory])?
                    } else {
                        serde_json::to_string_pretty(
                            inspector.steps.get(inspector.step).unwrap_or(&Vec::new()),
                        )?
                    };
                    fs::write(temp.path(), data)?;
                    let _ = Command::new("jless").arg(temp.path()).status();
                    crossterm::terminal::enable_raw_mode()?;
                    terminal.clear()?;
                }
                _ => {}
            }
        }
    };
    crossterm::terminal::disable_raw_mode()?;
    terminal.show_cursor()?;
    result
}

fn json_files(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?)
}

impl Inspector {
    fn load(&mut self) -> Result<()> {
        let data: Json = serde_json::from_str(&fs::read_to_string(&self.files[self.trajectory])?)?;
        self.messages = if let Some(messages) = data.get("messages").and_then(Value::as_array) {
            messages.clone()
        } else if data.is_array() {
            data.as_array().unwrap().clone()
        } else {
            return Err(mini_swe_agent::error::value_error(
                "Unrecognized trajectory format",
            ));
        };
        self.steps.clear();
        let mut current = Vec::new();
        for message in &self.messages {
            if message.pointer("/extra/actions").is_some()
                || message.get("role").and_then(Value::as_str) == Some("assistant")
            {
                self.steps.push(std::mem::take(&mut current));
                current.push(message.clone());
            } else {
                current.push(message.clone());
            }
        }
        if !current.is_empty() {
            self.steps.push(current);
        }
        self.step = self.steps.len().saturating_sub(1).min(self.step);
        self.scroll = 0;
        Ok(())
    }
    fn next_step(&mut self) {
        if self.step + 1 < self.steps.len() {
            self.step += 1;
            self.scroll = 0;
        }
    }
    fn prev_step(&mut self) {
        self.step = self.step.saturating_sub(1);
        self.scroll = 0;
    }
    fn next_trajectory(&mut self) {
        if self.trajectory + 1 < self.files.len() {
            self.trajectory += 1;
            self.step = 0;
            let _ = self.load();
        }
    }
    fn prev_trajectory(&mut self) {
        if self.trajectory > 0 {
            self.trajectory -= 1;
            self.step = 0;
            let _ = self.load();
        }
    }
    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let title = format!(
            "Trajectory {}/{} - {} - Step {}/{}",
            self.trajectory + 1,
            self.files.len(),
            self.files[self.trajectory]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            self.step + 1,
            self.steps.len()
        );
        let mut lines = Vec::new();
        if let Some(messages) = self.steps.get(self.step) {
            for message in messages {
                let role = message
                    .get("role")
                    .or_else(|| message.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                lines.push(Line::from(format!("\n{}", role.to_uppercase())));
                let content = get_content_string(message).replace('\0', "");
                lines.extend(content.lines().map(|line| Line::from(line.to_owned())));
                if self.reasoning && role == "assistant" {
                    if let Some(reasoning) =
                        message.get("reasoning_content").and_then(Value::as_str)
                    {
                        let reasoning = reasoning.replace('\0', "");
                        lines.push(Line::from("\nREASONING"));
                        lines.extend(reasoning.lines().map(|line| Line::from(line.to_owned())));
                    }
                }
            }
        }
        let block = Block::default().title(title).borders(Borders::ALL);
        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));
        frame.render_widget(paragraph, area);
    }
}

#[allow(dead_code)]
fn _rect(_: Option<Rect>) {}
