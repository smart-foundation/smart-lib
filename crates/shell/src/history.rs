use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

const MAX_HISTORY_SIZE: usize = 1000;
const HISTORY_FILE: &str = "history";

fn get_history_path() -> PathBuf {
    let mut path = dirs_config_dir();
    path.push(HISTORY_FILE);
    path
}

fn dirs_config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            home.push(".config");
            home
        })
}

fn ensure_config_dir() -> io::Result<PathBuf> {
    let mut path = dirs_config_dir();
    path.push("smart");
    if !path.exists() {
        fs::create_dir_all(&path)?;
    }
    Ok(path)
}

pub fn load_history() -> Vec<String> {
    let path = get_history_path();
    if !path.exists() {
        return Vec::new();
    }

    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = BufReader::new(file);
    let mut history = Vec::new();

    for line in reader.lines() {
        if let Ok(line) = line {
            if !line.is_empty() {
                history.push(line);
            }
        }
    }

    history
}

pub fn save_history(cmd: &str) {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return;
    }

    let mut history = load_history();

    if let Some(last) = history.last() {
        if last == trimmed {
            return;
        }
    }

    history.push(trimmed.to_string());

    if history.len() > MAX_HISTORY_SIZE {
        history = history.split_off(history.len() - MAX_HISTORY_SIZE);
    }

    let result = ensure_config_dir();
    let dir = match result {
        Ok(d) => d,
        Err(_) => return,
    };

    let mut path = dir;
    path.push(HISTORY_FILE);

    let file = match OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => return,
    };

    let mut file = file;
    for entry in &history {
        let _ = writeln!(file, "{}", entry);
    }
}

pub fn get_history(n: usize) -> Vec<String> {
    let history = load_history();
    let start = history.len().saturating_sub(n);
    history.into_iter().skip(start).collect()
}

pub fn clear_history() {
    let path = get_history_path();
    let _ = fs::remove_file(path);
}