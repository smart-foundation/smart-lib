//! Tab completion, repaired: the original shell BUILT this completer and never
//! wired it to rustyline. This one is wired (see `shell::run_shell`), and it
//! completes the real surface only — the domains that work today plus the
//! shell language's keywords — because help and completion list what is real,
//! never what is fake. The migration queue is data (`smart migrate`), not a
//! completion offer.

use std::collections::HashMap;

pub struct Completer {
    domains: Vec<String>,
    subcommands: HashMap<String, Vec<String>>,
    keywords: Vec<String>,
}

impl Completer {
    pub fn new() -> Self {
        let mut subcommands = HashMap::new();

        subcommands.insert("network".into(), vec![
            "status".into(), "interfaces".into(), "radios".into(),
            "interface".into(), "wifi".into(),
        ]);
        subcommands.insert("network wifi".into(), vec![
            "list".into(), "scan".into(), "connect".into(),
            "disconnect".into(), "forget".into(), "on".into(), "off".into(),
        ]);
        subcommands.insert("migrate".into(), vec![]);

        Completer {
            domains: vec!["network".into(), "migrate".into()],
            subcommands,
            keywords: vec![
                "help".into(), "exit".into(), "quit".into(),
                "watch".into(), "apply".into(), "export".into(),
                "where".into(), "sort".into(), "take".into(),
                "skip".into(), "count".into(), "reverse".into(),
                "unique".into(), "head".into(), "tail".into(),
                "pluck".into(), "group".into(), "flatten".into(),
                "len".into(), "first".into(), "last".into(),
                "sum".into(), "avg".into(), "min".into(), "max".into(),
                "if".into(), "elif".into(), "else".into(),
                "for".into(), "in".into(), "fn".into(),
                "true".into(), "false".into(), "null".into(),
                "@network".into(),
            ],
        }
    }

    pub fn complete(&self, input: &str) -> Vec<String> {
        let parts: Vec<&str> = input.split_whitespace().collect();

        // A trailing space means the user is completing a FRESH word: the
        // subcommands of the command prefix so far, not a prefix match.
        if input.ends_with(char::is_whitespace) {
            return match parts.as_slice() {
                [] | [_] => self
                    .subcommands
                    .get(parts.first().copied().unwrap_or(""))
                    .cloned()
                    .unwrap_or_default(),
                [domain, sub, ..] => self
                    .subcommands
                    .get(&format!("{domain} {sub}"))
                    .cloned()
                    .unwrap_or_default(),
            };
        }

        if parts.is_empty() {
            return self.domains.clone();
        }

        if parts.len() == 1 {
            let prefix = parts[0];
            let mut matches: Vec<String> = self.domains.iter()
                .chain(self.keywords.iter())
                .filter(|d| d.starts_with(prefix))
                .cloned()
                .collect();
            matches.sort();
            matches.dedup();
            return matches;
        }

        let domain = parts[0];
        let prefix = parts.last().unwrap_or(&"");

        // `network wifi <TAB>` completes the wifi subcommands.
        if parts.len() >= 3 && domain == "network" && parts[1] == "wifi" {
            if let Some(subs) = self.subcommands.get("network wifi") {
                let matches: Vec<String> = subs.iter()
                    .filter(|s| s.starts_with(prefix))
                    .cloned()
                    .collect();
                if !matches.is_empty() {
                    return matches;
                }
            }
        }

        if let Some(subs) = self.subcommands.get(domain) {
            let matches: Vec<String> = subs.iter()
                .filter(|s| s.starts_with(prefix))
                .cloned()
                .collect();
            if !matches.is_empty() {
                return matches;
            }
        }

        vec![]
    }
}

impl Default for Completer {
    fn default() -> Self {
        Self::new()
    }
}

pub fn complete_input(input: &str, completer: &Completer) -> Vec<String> {
    completer.complete(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_the_real_domains_and_keywords() {
        let c = Completer::new();
        assert!(c.complete("net").contains(&"network".to_string()));
        assert!(c.complete("mig").contains(&"migrate".to_string()));
        assert!(c.complete("wat").contains(&"watch".to_string()));
        // A domain that is only in the migration queue is not offered: help
        // and completion list what is real.
        assert!(c.complete("clip").is_empty());
    }

    #[test]
    fn completes_the_network_subcommands() {
        let c = Completer::new();
        let matches = c.complete("network ");
        for sub in ["status", "interfaces", "radios", "interface", "wifi"] {
            assert!(matches.contains(&sub.to_string()), "network completes {sub}");
        }
    }

    #[test]
    fn completes_the_wifi_subcommands() {
        let c = Completer::new();
        let matches = c.complete("network wifi ");
        for sub in ["list", "scan", "connect", "disconnect", "forget", "on", "off"] {
            assert!(matches.contains(&sub.to_string()), "wifi completes {sub}");
        }
    }

    #[test]
    fn completes_the_pipe_keywords() {
        let c = Completer::new();
        for kw in ["where", "sort", "take", "count", "group", "unique"] {
            assert!(c.complete("").contains(&kw.to_string()) || c.complete(kw).contains(&kw.to_string()));
        }
    }
}
