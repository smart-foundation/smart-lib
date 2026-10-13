pub enum Color {
    Reset,
    Red,
    Green,
    Yellow,
    Blue,
    Cyan,
    Bold,
    Dimmed,
}

impl Color {
    fn code(&self) -> &str {
        match self {
            Color::Reset => "\x1b[0m",
            Color::Red => "\x1b[31m",
            Color::Green => "\x1b[32m",
            Color::Yellow => "\x1b[33m",
            Color::Blue => "\x1b[34m",
            Color::Cyan => "\x1b[36m",
            Color::Bold => "\x1b[1m",
            Color::Dimmed => "\x1b[2m",
        }
    }
}

pub trait ColorExt {
    fn red(&self) -> String;
    fn green(&self) -> String;
    fn yellow(&self) -> String;
    fn blue(&self) -> String;
    fn cyan(&self) -> String;
    fn bold(&self) -> String;
    fn dimmed(&self) -> String;
}

impl ColorExt for str {
    fn red(&self) -> String { color(Color::Red, self) }
    fn green(&self) -> String { color(Color::Green, self) }
    fn yellow(&self) -> String { color(Color::Yellow, self) }
    fn blue(&self) -> String { color(Color::Blue, self) }
    fn cyan(&self) -> String { color(Color::Cyan, self) }
    fn bold(&self) -> String { color(Color::Bold, self) }
    fn dimmed(&self) -> String { color(Color::Dimmed, self) }
}

impl ColorExt for String {
    fn red(&self) -> String { color(Color::Red, self) }
    fn green(&self) -> String { color(Color::Green, self) }
    fn yellow(&self) -> String { color(Color::Yellow, self) }
    fn blue(&self) -> String { color(Color::Blue, self) }
    fn cyan(&self) -> String { color(Color::Cyan, self) }
    fn bold(&self) -> String { color(Color::Bold, self) }
    fn dimmed(&self) -> String { color(Color::Dimmed, self) }
}

pub fn color(c: Color, text: &str) -> String {
    format!("{}{}{}", c.code(), text, Color::Reset.code())
}

pub fn success(msg: &str) {
    println!("{}", color(Color::Green, msg));
}

pub fn error(msg: &str) {
    eprintln!("{}", color(Color::Red, msg));
}

pub fn warning(msg: &str) {
    eprintln!("{}", color(Color::Yellow, msg));
}

pub fn info(msg: &str) {
    println!("{}", color(Color::Cyan, msg));
}

pub fn heading(msg: &str) {
    println!("{}", color(Color::Bold, msg));
}

pub fn is_tty() -> bool {
    atty::is(atty::Stream::Stdout)
}