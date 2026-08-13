use comfy_table::{Cell, Table};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Envelope {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub dry_run: bool,
    #[serde(rename = "results")]
    pub accounts: Vec<AccountOutcome>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountOutcome {
    pub account: String,
    pub ok: bool,
    pub error: Option<serde_json::Value>,
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing)]
    pub exit_code: Option<i32>,
}

impl Envelope {
    pub fn new(accounts: Vec<AccountOutcome>, dry_run: bool) -> Self {
        Envelope {
            ok: accounts.iter().all(|a| a.ok),
            command: None,
            dry_run,
            accounts,
        }
    }
}

pub fn log_line(level: &str, message: &str) {
    let min = crate::logging::min_line_level();
    let lv = match level {
        "error" => 3,
        "warn" => 2,
        "info" => 1,
        _ => 0,
    };
    if lv < min {
        return;
    }
    eprintln!("[{level}] {message}");
}

pub fn print_json(value: &serde_json::Value) {
    println!("{}", serde_json::to_string(value).expect("serialize"));
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut table = Table::new();
    table.set_header(headers.iter().map(|h| Cell::new(*h)));
    for row in rows {
        table.add_row(row.iter().map(Cell::new));
    }
    println!("{table}");
}

pub fn machine_mode(json: bool, jsonl: bool) -> bool {
    json || jsonl
}
