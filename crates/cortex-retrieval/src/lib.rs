//! Semantic code search.
//!
//! Plain text search finds code when you know a word that appears in it.
//! A bug report rarely contains such a word: "evening trades show up on the
//! wrong day" shares no identifier with the function at fault. An embedding
//! model maps both the report and the code to vectors that are close when
//! they are about the same thing, so the right code can be found from a
//! description.
//!
//! - [`chunker`] cuts source files into function-sized pieces.
//! - [`client`] asks the local embedding service (see `ml/`) for vectors.
//! - [`search`] keeps the vectors for a project and ranks them for a query.
//! - [`quant`] is the small form the vectors are kept in.
//! - [`store`] saves the vectors in the project between runs.

pub mod chunker;
pub mod client;
pub mod quant;
pub mod search;
mod store;

pub use chunker::Chunk;
pub use client::{EmbeddingClient, EmbeddingError};
pub use search::{Hit, IndexSize, SemanticIndex};
