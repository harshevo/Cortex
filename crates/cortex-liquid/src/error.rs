//! What can go wrong, said so that a person can act on it.

/// Why bars could not be read or analysed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("the file is empty")]
    Empty,

    #[error("no column for {wanted}: expected one of {expected}, found the columns {found}")]
    MissingColumn {
        /// What the column holds, such as "the closing price".
        wanted: String,

        /// The names that would have been recognised.
        expected: String,

        /// The names the file has.
        found: String,
    },

    #[error("no usable rows: {skipped} were skipped, the first because {problem}")]
    NoRows { skipped: usize, problem: String },

    #[error("too few bars: {found} given, at least {needed} needed for these settings")]
    TooFewBars { needed: usize, found: usize },

    #[error("bar {index} cannot be used: {problem}")]
    BadBar { index: usize, problem: String },

    #[error("settings cannot be used: {0}")]
    BadSettings(String),

    #[error("the saved model cannot be read: {0}")]
    Saved(String),
}
