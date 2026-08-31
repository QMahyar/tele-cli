use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

use grammers_tl_parser::parse_tl_file;
use grammers_tl_parser::tl::{Category, ParameterType};

const REQUIRED_INTS: &[(&str, &[&str])] = &[
    ("contacts.Search", &["limit"]),
    ("messages.AppendTodoList", &["msg_id"]),
    ("messages.ToggleTodoCompleted", &["msg_id"]),
    ("messages.TranscribeAudio", &["msg_id"]),
];

const REGISTRY: &[&str] = &[
    "account.GetAuthorizations",
    "account.SetAuthorizationTTL",
    "account.UpdateProfile",
    "channels.GetFullChannel",
    "contacts.DeleteByPhones",
    "contacts.Search",
    "messages.AppendTodoList",
    "messages.ComposeMessageWithAI",
    "messages.ExportChatInvite",
    "messages.GetAllDrafts",
    "messages.GetAvailableEffects",
    "messages.GetDialogUnreadMarks",
    "messages.GetHistory",
    "messages.GetMessagesViews",
    "messages.GetScheduledHistory",
    "messages.ReadMentions",
    "messages.ReadReactions",
    "messages.Search",
    "messages.SendScheduledMessages",
    "messages.ToggleTodoCompleted",
    "messages.TranslateText",
    "messages.TranscribeAudio",
    "stats.GetBroadcastStats",
    "stats.GetMegagroupStats",
    "users.GetUsers",
];

const NEEDS_PEER_RESOLVE: &[&str] = &[
    "channels.GetFullChannel",
    "messages.AppendTodoList",
    "messages.ExportChatInvite",
    "messages.GetHistory",
    "messages.GetMessagesViews",
    "messages.GetScheduledHistory",
    "messages.ReadMentions",
    "messages.ReadReactions",
    "messages.Search",
    "messages.SendScheduledMessages",
    "messages.ToggleTodoCompleted",
    "messages.TranslateText",
    "messages.TranscribeAudio",
    "stats.GetBroadcastStats",
    "stats.GetMegagroupStats",
    "users.GetUsers",
];

// TL parameter name → CLI-facing alias
const PARAM_ALIASES: &[(&str, &str, &str)] = &[
    ("messages.ExportChatInvite", "peer", "chat"),
    ("messages.GetHistory", "peer", "chat"),
    ("messages.GetScheduledHistory", "peer", "chat"),
    ("messages.Search", "peer", "chat"),
    ("messages.GetMessagesViews", "peer", "chat"),
    ("messages.ReadReactions", "peer", "chat"),
    ("messages.ReadMentions", "peer", "chat"),
    ("messages.AppendTodoList", "peer", "chat"),
    ("messages.SendScheduledMessages", "peer", "chat"),
    ("messages.ToggleTodoCompleted", "peer", "chat"),
    ("messages.TranslateText", "peer", "chat"),
    ("messages.TranscribeAudio", "peer", "chat"),
];

fn main() {
    println!("cargo:rerun-if-changed=tl/api.tl");
    println!("cargo:rerun-if-changed=build.rs");

    let contents = fs::read_to_string("tl/api.tl").expect("failed to read tl/api.tl");
    let mut methods: BTreeMap<String, Vec<(String, String, bool, bool)>> = BTreeMap::new();

    for result in parse_tl_file(&contents) {
        let def = match result {
            Ok(d) => d,
            Err(_) => continue,
        };
        if def.category != Category::Functions {
            continue;
        }
        let tl_name = def.full_name();
        let full_name: String = {
            let parts: Vec<&str> = tl_name.split('.').collect();
            if parts.len() <= 1 {
                tl_name.clone()
            } else {
                let mut result = parts[..parts.len() - 1].join(".");
                let method = parts.last().unwrap();
                let mut c = method.chars();
                match c.next() {
                    None => result,
                    Some(f) => {
                        result.push('.');
                        result.push_str(&f.to_uppercase().collect::<String>());
                        result.push_str(c.as_str());
                        result
                    }
                }
            }
        };
        if !REGISTRY.contains(&full_name.as_str()) {
            continue;
        }
        let mut params = Vec::new();
        for param in &def.params {
            match &param.ty {
                ParameterType::Flags => {
                    params.push((param.name.clone(), "flags".to_string(), false, false));
                }
                ParameterType::Normal { ty, flag } => {
                    let tl_type = ty.to_string();
                    let is_optional = flag.is_some();
                    let is_bool_flag = tl_type == "true";
                    params.push((param.name.clone(), tl_type, is_optional, is_bool_flag));
                }
            }
        }
        methods.insert(full_name, params);
    }

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("generated_raw.rs");
    let mut f = fs::File::create(&dest_path).unwrap();

    // ── registry module ──────────────────────────────────────────────
    writeln!(f, "pub mod registry {{").unwrap();
    writeln!(f, "    #[allow(dead_code)]").unwrap();
    writeln!(f, "    pub struct ArgMeta {{").unwrap();
    writeln!(f, "        pub name: &'static str,").unwrap();
    writeln!(f, "        pub tl_type: &'static str,").unwrap();
    writeln!(f, "        pub optional: bool,").unwrap();
    writeln!(f, "        pub bool_flag: bool,").unwrap();
    writeln!(f, "    }}").unwrap();
    writeln!(f, "    #[allow(dead_code)]").unwrap();
    writeln!(f, "    pub struct MethodMeta {{").unwrap();
    writeln!(f, "        pub name: &'static str,").unwrap();
    writeln!(f, "        pub args: &'static [ArgMeta],").unwrap();
    writeln!(f, "        pub needs_peer: bool,").unwrap();
    writeln!(f, "        pub required_ints: &'static [&'static str],").unwrap();
    writeln!(f, "    }}").unwrap();
    writeln!(f, "    pub const NO_REQUIRED_INTS: &[&str] = &[];").unwrap();
    writeln!(f, "    pub static METHODS: &[MethodMeta] = &[").unwrap();
    for name in REGISTRY {
        let args = methods.get(*name).unwrap();
        let needs = NEEDS_PEER_RESOLVE.contains(name);
        writeln!(f, "        MethodMeta {{").unwrap();
        writeln!(f, "            name: {:?},", name).unwrap();
        writeln!(f, "            args: &[").unwrap();
        for (pname, tl_type, optional, bool_flag) in args {
            let display_name = PARAM_ALIASES
                .iter()
                .find(|(m, tl, _)| **m == **name && *tl == pname.as_str())
                .map_or(pname.as_str(), |a| a.2);
            writeln!(
                f,
                "                ArgMeta {{ name: {:?}, tl_type: {:?}, optional: {}, bool_flag: {} }},",
                display_name, tl_type, optional, bool_flag
            )
            .unwrap();
        }
        writeln!(f, "            ],").unwrap();
        let required_ints = REQUIRED_INTS
            .iter()
            .find(|(m, _)| *m == *name)
            .map(|(_, fields)| *fields)
            .unwrap_or(&[]);
        writeln!(f, "            needs_peer: {},", needs).unwrap();
        if required_ints.is_empty() {
            writeln!(f, "            required_ints: NO_REQUIRED_INTS,").unwrap();
        } else {
            writeln!(f, "            required_ints: &{:?},", required_ints).unwrap();
        }
        writeln!(f, "        }},").unwrap();
    }
    writeln!(f, "    ];").unwrap();
    writeln!(
        f,
        "    pub fn lookup(name: &str) -> Option<&'static MethodMeta> {{"
    )
    .unwrap();
    writeln!(f, "        METHODS.iter().find(|m| m.name == name)").unwrap();
    writeln!(f, "    }}").unwrap();
    writeln!(f, "}}").unwrap();

    // ── generated validation ─────────────────────────────────────────
    writeln!(f).unwrap();
    writeln!(f, "use crate::error::{{TeleError, TeleResult}};").unwrap();
    writeln!(f).unwrap();
    writeln!(
        f,
        "pub fn validate_params(name: &str, p: &serde_json::Value) -> TeleResult<()> {{"
    )
    .unwrap();
    writeln!(
        f,
        r#"    if !p.is_object() {{ return Err(TeleError::Usage("--args must be a JSON object of constructor kwargs".to_string())); }}"#
    )
    .unwrap();
    writeln!(
        f,
        "    let method = registry::lookup(name).ok_or_else(|| {{"
    )
    .unwrap();
    writeln!(
        f,
        "        TeleError::Usage(format!(\"raw method not in registry; add an arm in src/commands/raw.rs (registered: {{:?}})\", registry::METHODS.iter().map(|m| m.name).collect::<Vec<_>>()))"
    )
    .unwrap();
    writeln!(f, "    }})?;").unwrap();
    writeln!(f, "    for key in p.as_object().unwrap().keys() {{").unwrap();
    writeln!(
        f,
        "        if !method.args.iter().any(|a| a.name == key.as_str()) {{"
    )
    .unwrap();
    writeln!(
        f,
        "            let valid: Vec<&str> = method.args.iter().map(|a| a.name).collect();"
    )
    .unwrap();
    writeln!(
        f,
        r#"            return Err(TeleError::Usage(format!("unknown --args key(s) [{{key}}] for {{name}} (valid keys: {{valid:?}})")));"#
    )
    .unwrap();
    writeln!(f, "        }}").unwrap();
    writeln!(f, "    }}").unwrap();
    writeln!(f, "    for arg in method.args {{").unwrap();
    writeln!(f, "        if arg.tl_type == \"flags\" {{ continue; }}").unwrap();
    writeln!(f, "        match p.get(arg.name) {{").unwrap();
    writeln!(f, "            None => {{").unwrap();
    writeln!(f, "                if !arg.optional {{").unwrap();
    writeln!(
        f,
        "                    if method.required_ints.contains(&arg.name) {{"
    )
    .unwrap();
    writeln!(
        f,
        r#"                        return Err(TeleError::Usage(format!("--args field {{}} is required (integer)", arg.name)));"#
    )
    .unwrap();
    writeln!(
        f,
        "                    }} else if arg.tl_type == \"int\" || arg.tl_type == \"long\" {{"
    )
    .unwrap();
    writeln!(f, "                    }} else {{").unwrap();
    writeln!(
        f,
        r#"                        return Err(TeleError::Usage(format!("--args field {{}} is required (non-empty string)", arg.name)));"#
    )
    .unwrap();
    writeln!(f, "                    }}").unwrap();
    writeln!(f, "                }}").unwrap();
    writeln!(f, "            }}").unwrap();
    writeln!(f, "            Some(val) => {{").unwrap();
    // bool (flags.0?true)
    writeln!(f, "                if arg.tl_type == \"true\" {{").unwrap();
    writeln!(
        f,
        r#"                    if !val.is_boolean() {{ return Err(TeleError::Usage(format!("--args field {{}} must be a boolean", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, "                }}").unwrap();
    writeln!(
        f,
        r#"                else if arg.tl_type == "InputPeer" || arg.tl_type == "InputUser" || arg.tl_type == "InputChannel" {{"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    if !val.is_string() && !val.is_number() {{ return Err(TeleError::Usage(format!("--args field {{}} must be a string or integer", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, r#"                    if let Some(s) = val.as_str() {{"#).unwrap();
    writeln!(
        f,
        r#"                        if s.trim().is_empty() {{ return Err(TeleError::Usage(format!("--args field {{}} is required (non-empty string)", arg.name))); }}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                        if s.len() > 4096 {{ return Err(TeleError::Usage(format!("--args field {{}} exceeds 4096 characters", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, r#"                    }}"#).unwrap();
    writeln!(f, "                }}").unwrap();
    writeln!(
        f,
        r#"                else if arg.tl_type == "Vector<InputUser>" || arg.tl_type == "Vector<InputPeer>" || arg.tl_type == "Vector<InputChannel>" {{"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    let arr = val.as_array().ok_or_else(|| TeleError::Usage(format!("--args field {{}} must be an array", arg.name)))?;"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    if arr.is_empty() {{ return Err(TeleError::Usage(format!("--args field {{}} must be a non-empty array", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, r#"                    for item in arr {{"#).unwrap();
    writeln!(
        f,
        r#"                        if !item.is_string() && !item.is_number() {{ return Err(TeleError::Usage(format!("--args field {{}} must contain only strings or integers", arg.name))); }}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                        if let Some(s) = item.as_str() {{"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                            if s.trim().is_empty() {{ return Err(TeleError::Usage(format!("--args field {{}} must contain non-empty strings", arg.name))); }}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                            if s.len() > 4096 {{ return Err(TeleError::Usage(format!("--args field {{}} exceeds 4096 characters", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, r#"                        }}"#).unwrap();
    writeln!(f, r#"                    }}"#).unwrap();
    writeln!(f, "                }}").unwrap();
    writeln!(
        f,
        r#"                else if arg.tl_type == "string" || arg.tl_type == "bytes" || arg.tl_type.starts_with("Text") || arg.tl_type == "InputPhoto" || arg.tl_type == "InputFile" || arg.tl_type == "InputMedia" || arg.tl_type == "InputWebFile" {{"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    if !val.is_string() {{ return Err(TeleError::Usage(format!("--args field {{}} must be a string", arg.name))); }}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    if val.as_str().is_some_and(|s| s.trim().is_empty()) {{ return Err(TeleError::Usage(format!("--args field {{}} is required (non-empty string)", arg.name))); }}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    if val.as_str().is_some_and(|s| s.len() > 4096) {{ return Err(TeleError::Usage(format!("--args field {{}} exceeds 4096 characters", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, "                }}").unwrap();
    writeln!(
        f,
        r#"                else if arg.tl_type == "Vector<string>" || arg.tl_type == "Vector<bytes>" {{"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    let arr = val.as_array().ok_or_else(|| TeleError::Usage(format!("--args field {{}} must be an array", arg.name)))?;"#
    )
    .unwrap();
    writeln!(f, r#"                    for item in arr {{"#).unwrap();
    writeln!(
        f,
        r#"                        let s = item.as_str().ok_or_else(|| TeleError::Usage(format!("--args field {{}} must contain only strings", arg.name)))?;"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                        if s.trim().is_empty() {{ return Err(TeleError::Usage(format!("--args field {{}} must contain non-empty strings", arg.name))); }}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                        if s.len() > 4096 {{ return Err(TeleError::Usage(format!("--args field {{}} exceeds 4096 characters", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, r#"                    }}"#).unwrap();
    writeln!(f, "                }}").unwrap();
    // int
    writeln!(f, r#"                else if arg.tl_type == "int" {{"#).unwrap();
    writeln!(
        f,
        r#"                    let n = val.as_i64().ok_or_else(|| TeleError::Usage(format!("--args field {{}} must be an integer", arg.name)))?;"#
    )
    .unwrap();
    writeln!(
        f,
        r#"                    i32::try_from(n).map_err(|_| TeleError::Usage(format!("--args field {{}} is out of range", arg.name)))?;"#
    )
    .unwrap();
    writeln!(f, "                }}").unwrap();
    // long
    writeln!(
        f,
        r#"                else if arg.tl_type == "long" && val.as_i64().is_none() {{ return Err(TeleError::Usage(format!("--args field {{}} must be an integer", arg.name))); }}"#
    )
    .unwrap();
    writeln!(f, "            }}").unwrap();
    writeln!(f, "        }}").unwrap();
    writeln!(f, "    }}").unwrap();
    writeln!(f, "    Ok(())").unwrap();
    writeln!(f, "}}").unwrap();

    // ── helper functions used by dispatch ────────────────────────────
    writeln!(f).unwrap();
    writeln!(
        f,
        "pub fn requires_explicit_account(method: &str) -> bool {{"
    )
    .unwrap();
    writeln!(
        f,
        "    matches!(method, \"account.UpdateProfile\" | \"account.SetAuthorizationTTL\" | \"contacts.DeleteByPhones\" | \"messages.ExportChatInvite\" | \"messages.AppendTodoList\" | \"messages.SendScheduledMessages\" | \"messages.ToggleTodoCompleted\")"
    )
    .unwrap();
    writeln!(f, "}}").unwrap();
}
