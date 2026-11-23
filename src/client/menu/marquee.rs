use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

pub(super) const INTERVAL: Duration = Duration::from_millis(350);
const WINDOW: usize = 40;
const GAP: &str = "   ";

#[derive(Default)]
pub(super) struct Marquee {
    text: String,
    starts: Vec<usize>,
    offset: usize,
}

impl Marquee {
    pub(super) fn set(&mut self, text: String) -> Option<String> {
        if self.text == text {
            return None;
        }
        let mut starts: Vec<_> = text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .collect();
        if starts.len() > WINDOW {
            starts.extend(text.len()..text.len() + GAP.len());
        } else {
            starts.clear();
        }
        *self = Self {
            text,
            starts,
            offset: 0,
        };
        Some(self.frame())
    }

    pub(super) fn advance(&mut self) -> Option<String> {
        if self.starts.is_empty() {
            return None;
        }
        self.offset = (self.offset + 1) % self.starts.len();
        Some(self.frame())
    }

    fn frame(&self) -> String {
        if self.starts.is_empty() {
            return self.text.clone();
        }
        let cycle = format!("{}{GAP}", self.text);
        self.starts
            .iter()
            .enumerate()
            .cycle()
            .skip(self.offset)
            .take(WINDOW)
            .map(|(index, &start)| {
                let end = self.starts.get(index + 1).copied().unwrap_or(cycle.len());
                &cycle[start..end]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_by_grapheme_and_restarts_only_when_text_changes() {
        let prefix = "👩‍🎤";
        let tail = "曲".repeat(WINDOW - 1);
        let text = format!("{prefix}{tail}終");
        let mut marquee = Marquee::default();
        assert_eq!(marquee.set(text.clone()), Some(format!("{prefix}{tail}")));
        assert_eq!(marquee.set(text), None);
        assert_eq!(marquee.advance(), Some(format!("{tail}終")));
        assert_eq!(
            marquee.advance(),
            Some(format!("{}終 ", "曲".repeat(WINDOW - 2)))
        );
        for _ in 0..WINDOW + 2 {
            marquee.advance();
        }
        assert_eq!(marquee.frame(), format!("{prefix}{tail}"));
        assert_eq!(
            marquee.set("Short title".into()).as_deref(),
            Some("Short title")
        );
        assert_eq!(marquee.advance(), None);
    }
}
