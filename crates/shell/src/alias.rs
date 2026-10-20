use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn get_aliases_path() -> PathBuf {
    let mut path = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            home.push(".config");
            home
        });
    path.push("smart/aliases.toml");
    path
}

pub fn load_aliases() -> HashMap<String, String> {
    let path = get_aliases_path();
    if !path.exists() {
        return default_aliases();
    }

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return default_aliases(),
    };

    match toml::from_str::<TomlAliases>(&content) {
        Ok(aliases) => aliases.aliases,
        Err(_) => default_aliases(),
    }
}

#[derive(serde::Deserialize)]
struct TomlAliases {
    #[serde(default)]
    aliases: HashMap<String, String>,
}

fn default_aliases() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("sw".to_string(), "switch-workspace".to_string());
    m.insert("v".to_string(), "volume".to_string());
    m.insert("p".to_string(), "power".to_string());
    m.insert("d".to_string(), "display".to_string());
    m.insert("w".to_string(), "window".to_string());
    m
}

pub fn resolve_alias(cmd: &str, aliases: &HashMap<String, String>) -> String {
    let mut parts = cmd.split_whitespace();
    let first = match parts.next() {
        Some(p) => p,
        None => return cmd.to_string(),
    };

    if let Some(replacement) = aliases.get(first) {
        let rest: String = parts.collect::<Vec<_>>().join(" ");
        if rest.is_empty() {
            replacement.clone()
        } else {
            format!("{} {}", replacement, rest)
        }
    } else {
        cmd.to_string()
    }
}

pub fn expand_shorthand(cmd: &str) -> String {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return cmd.to_string();
    }

    let first = parts[0];
    let rest: String = if parts.len() > 1 {
        parts[1..].join(" ")
    } else {
        String::new()
    };

    // Handle plurals and special commands
    let expanded = match first {
        // Plurals
        "windows" => "window",
        "processes" => "process",
        "displays" => "display",
        "networks" => "network",
        "files" if rest.is_empty() => "filesystem", // bare "files" is ambiguous, use filesystem
        
        // Domain shorthands
        "vol" => "volume",
        "pk" | "pkg" => "package",
        "win" => "window",
        "svc" => "service",
        "bk" | "backup" => "backup",
        "upd" => "update",
        "net" => "network",
        "disp" => "display",
        "audio" | "aud" => "audio",
        "power" | "pw" => "power",
        "proc" => "process",
        "sys" => "system",
        "fs" | "file" if rest.is_empty() => "filesystem",
        "fs" | "file" => "filesystem", // with args
        "clip" => "clipboard",
        "notif" => "notification",
        "print" | "prn" => "print",
        "cam" => "camera",
        "speech" | "tts" => "speech",
        "trans" => "translation",
        "cal" => "calendar",
        "cont" => "contacts",
        "loc" => "location",
        "capt" => "capture",
        "acc" => "accessibility",
        "auth" => "auth",
        "buff" => "buffer",
        "surf" => "surface",
        "comp" => "compositor",
        "sess" => "session",
        "search" | "srch" => "search",
        "web" => "web",
        "media" | "med" => "media",
        "theme" | "thm" => "theme",
        "font" | "fnt" => "font",
        "time" | "tm" => "time",
        "settings" | "set" => "settings",
        "secret" | "sec" => "secret",
        "db" | "database" => "database",
        "q" | "queue" => "queue",
        "mf" | "manifest" => "manifest",
        "vc" | "vcard" => "vcard",
        "cnt" | "content" => "content",
        "app" | "application" => "app",
        "idx" | "index" => "appindex",
        "ext" => "extension",
        "usb" => "usb",
        "logging" | "log" => "logging",
        "input" | "inp" => "input",
        "textinput" | "txtinp" => "textinput",
        _ => first,
    };

    if rest.is_empty() {
        expanded.to_string()
    } else {
        format!("{} {}", expanded, rest)
    }
}

pub fn process_command_input(cmd: &str, aliases: &HashMap<String, String>) -> String {
    let expanded = expand_shorthand(cmd);
    resolve_alias(&expanded, aliases)
}