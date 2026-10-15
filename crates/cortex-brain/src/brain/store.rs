//! Atomic versioned checkpoints contain both cortex and mind. A JSON
//! sidecar exports inspectable state; legacy split checkpoints still load.

use super::*;

const MAGIC: &[u8] = b"CORTEX-BRAIN\x02\n";

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    brain: Brain,
    mind: Mind,
    budget: u64,
}

#[derive(Serialize)]
struct CheckpointRef<'a> {
    brain: &'a Brain,
    mind: &'a Mind,
    budget: u64,
}

impl Brain {
    pub fn load(file: &Path) -> std::io::Result<Self> {
        let bytes = fs::read(file)?;
        let current = bytes.starts_with(MAGIC);
        let mut brain: Brain = if current {
            let saved: Checkpoint =
                postcard::from_bytes(&bytes[MAGIC.len()..]).map_err(std::io::Error::other)?;
            let mut brain = saved.brain;
            brain.mind = saved.mind;
            brain.budget = saved.budget;
            brain
        } else {
            postcard::from_bytes(&bytes).map_err(std::io::Error::other)?
        };
        if !brain.is_sound() {
            return Err(std::io::Error::other(
                "the saved brain is not in working order",
            ));
        }
        // A brain saved before it had a mind starts one now.
        if !current
            && let Ok(text) = fs::read_to_string(mind_file(file))
            && let Ok(mind) = serde_json::from_str(&text)
        {
            brain.mind = mind;
        }
        if brain.mind.cognition.knowledge == knowledge::Knowledge::default() {
            for engram in &brain.engrams {
                brain.mind.cognition.knowledge.learn(
                    &engram.text(),
                    &engram.source,
                    engram.id,
                    false,
                );
            }
        }
        brain.mind.map.fit(brain.cortex.area());
        // A map never organised gets a long first pass, so that related
        // columns sit together from the first look rather than after
        // many nights.
        if brain.mind.map.swaps == 0 {
            for _ in 0..10 {
                brain.organise();
            }
        }
        Ok(brain)
    }

    /// Written beside the file and moved over it.
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        let folder = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(folder)?;
        let bytes = postcard::to_stdvec(&CheckpointRef {
            brain: self,
            mind: &self.mind,
            budget: self.budget(),
        })
        .map_err(std::io::Error::other)?;
        let mut partial = tempfile::NamedTempFile::new_in(folder)?;
        partial.write_all(MAGIC)?;
        partial.write_all(&bytes)?;
        partial.as_file().sync_all()?;
        partial.persist(file).map_err(|e| e.error)?;
        // The JSON is an inspectable export. The atomically written binary
        // above is authoritative, including the mind and conversation state.
        let mind = serde_json::to_vec(&self.mind).map_err(std::io::Error::other)?;
        let mut partial = tempfile::NamedTempFile::new_in(folder)?;
        partial.write_all(&mind)?;
        partial.as_file().sync_all()?;
        partial.persist(mind_file(file)).map_err(|e| e.error)?;
        Ok(())
    }
}

/// Where the mind is kept, beside the brain's file.
pub fn mind_file(file: &Path) -> std::path::PathBuf {
    file.with_extension("mind.json")
}
