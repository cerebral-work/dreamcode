//! Category enum + default filter + serde round-trip for settings.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    MemoryIo,
    Dream,
    Tx,
    Coord,
    Gate,
    Permission,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::MemoryIo,
        Category::Dream,
        Category::Tx,
        Category::Coord,
        Category::Gate,
        Category::Permission,
    ];

    pub fn wire_name(&self) -> &'static str {
        match self {
            Category::MemoryIo => "memory-io",
            Category::Dream => "dream",
            Category::Tx => "tx",
            Category::Coord => "coord",
            Category::Gate => "gate",
            Category::Permission => "permission",
        }
    }

    pub fn display_name(&self) -> &'static str {
        self.wire_name()
    }

    pub fn from_wire(s: &str) -> Option<Category> {
        Category::ALL.iter().copied().find(|c| c.wire_name() == s)
    }
}

#[derive(Debug, Clone)]
pub struct CategoryFilter {
    enabled: HashSet<Category>,
}

impl Default for CategoryFilter {
    fn default() -> Self {
        let mut enabled = HashSet::new();
        enabled.insert(Category::MemoryIo);
        enabled.insert(Category::Dream);
        Self { enabled }
    }
}

impl CategoryFilter {
    pub fn is_enabled(&self, c: Category) -> bool {
        self.enabled.contains(&c)
    }

    pub fn toggle(&mut self, c: Category) {
        if !self.enabled.insert(c) {
            self.enabled.remove(&c);
        }
    }

    pub fn enabled_set(&self) -> &HashSet<Category> {
        &self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_memory_io_and_dream() {
        let f = CategoryFilter::default();
        assert!(f.is_enabled(Category::MemoryIo));
        assert!(f.is_enabled(Category::Dream));
        assert!(!f.is_enabled(Category::Tx));
        assert!(!f.is_enabled(Category::Coord));
    }

    #[test]
    fn toggle_flips_membership() {
        let mut f = CategoryFilter::default();
        assert!(!f.is_enabled(Category::Tx));
        f.toggle(Category::Tx);
        assert!(f.is_enabled(Category::Tx));
        f.toggle(Category::Tx);
        assert!(!f.is_enabled(Category::Tx));
    }

    #[test]
    fn wire_name_round_trip() {
        for c in Category::ALL {
            assert_eq!(Category::from_wire(c.wire_name()), Some(c));
        }
        assert_eq!(Category::from_wire("nonsense"), None);
    }
}
