use std::fmt;

pub struct ProgressBar {
    total: u64,
    current: u64,
    width: usize,
}

impl ProgressBar {
    pub fn new(total: u64) -> Self {
        ProgressBar {
            total,
            current: 0,
            width: 40,
        }
    }

    pub fn set(&mut self, current: u64) {
        self.current = current.min(self.total);
    }

    pub fn inc(&mut self, delta: u64) {
        self.current = (self.current + delta).min(self.total);
    }

    pub fn finish(mut self) {
        self.current = self.total;
        println!("{}", self);
    }
}

impl fmt::Display for ProgressBar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let percent = if self.total == 0 {
            100.0
        } else {
            (self.current as f64 / self.total as f64) * 100.0
        };

        let filled = if self.total == 0 {
            self.width
        } else {
            ((self.current as f64 / self.total as f64) * self.width as f64).round() as usize
        };

        let empty = self.width.saturating_sub(filled);

        write!(
            f,
            "\r[{}{}] {:.0}%  {}/{}",
            "█".repeat(filled),
            "░".repeat(empty),
            percent,
            self.current,
            self.total
        )
    }
}

impl Default for ProgressBar {
    fn default() -> Self {
        Self::new(100)
    }
}

pub struct MultiProgress {
    bars: Vec<ProgressBar>,
}

impl MultiProgress {
    pub fn new(count: usize) -> Self {
        MultiProgress {
            bars: (0..count).map(|_| ProgressBar::new(100)).collect(),
        }
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut ProgressBar> {
        self.bars.get_mut(index)
    }

    pub fn finish_all(self) {
        for bar in self.bars {
            println!("{}", bar);
        }
    }
}