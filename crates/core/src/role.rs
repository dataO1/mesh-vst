//! A lane's role: which generator conventions apply (drum kit or pitched).

/// A lane's role. The plugin plays one lane per instance: drums or bass.
/// Lead and Extra stay because the copied generators still match on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Role {
    Drums,
    #[default]
    Bass,
    Lead,
    Extra,
}
