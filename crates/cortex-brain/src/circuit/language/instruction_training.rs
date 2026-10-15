use super::*;
use std::fs;

impl Assistant {
    pub fn cache_instruction_features(
        &self,
        specification: &serde_json::Value,
        directory: &Path,
    ) -> Result<(), String> {
        #[derive(Deserialize, Serialize)]
        #[serde(deny_unknown_fields)]
        struct Example {
            id: String,
            group: String,
            split: String,
            question: String,
            answer: String,
        }
        let rows: Vec<Example> =
            serde_json::from_value(specification.clone()).map_err(|e| e.to_string())?;
        if rows.len() > 512 || self.network.neurons().len() > 8192 {
            return Err("Instruction cache exceeds budget".into());
        }
        let dialogue = self
            .dialogue
            .as_ref()
            .ok_or("Need the existing trained tokenizer and language head")?;
        let mut groups = BTreeMap::new();
        let mut questions = BTreeMap::new();
        for row in &rows {
            if !matches!(row.split.as_str(), "train" | "dev" | "test")
                || row.question.is_empty()
                || row.answer.is_empty()
                || row.answer.len() > 1024
            {
                return Err("Invalid instruction record".into());
            }
            for (seen, key) in [
                (&mut groups, row.group.clone()),
                (&mut questions, row.question.trim().to_lowercase()),
            ] {
                if let Some(previous) = seen.insert(key, row.split.clone())
                    && previous != row.split
                {
                    return Err("Instruction split leakage".into());
                }
            }
        }
        fs::create_dir(directory).map_err(|e| e.to_string())?;
        let mut features = BufWriter::new(
            fs::File::create(directory.join("features.f32")).map_err(|e| e.to_string())?,
        );
        let mut targets = BufWriter::new(
            fs::File::create(directory.join("targets.u16")).map_err(|e| e.to_string())?,
        );
        let mut index = Vec::new();
        let mut offset = 0;
        for row in rows {
            let mut network = self.network.detached();
            dialogue.question(&mut network, &row.question, false)?;
            let mut tokens = dialogue.encode(&row.answer);
            tokens.push(0);
            if tokens.len() > 256 {
                return Err("Instruction answer exceeds 256 symbols".into());
            }
            for token in &tokens {
                for feature in dialogue.language_features(&network) {
                    features
                        .write_all(&feature.to_le_bytes())
                        .map_err(|e| e.to_string())?;
                }
                targets
                    .write_all(&(*token as u16).to_le_bytes())
                    .map_err(|e| e.to_string())?;
                dialogue.feed(&mut network, *token)?;
            }
            index.push(serde_json::json!({"record":row,"offset":offset,"length":tokens.len()}));
            offset += tokens.len();
        }
        features.flush().map_err(|e| e.to_string())?;
        targets.flush().map_err(|e| e.to_string())?;
        let file = fs::File::create(directory.join("index.json")).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(file,&serde_json::json!({"rows":offset,"features":self.network.neurons().len(),"vocabulary":dialogue.tokens.len(),"core_signature":super::self_monitor::signature(&self.network),"examples":index,"scope":"Frozen-core teacher-forced answer-token features; source answers only"})).map_err(|e|e.to_string())?;
        Ok(())
    }
}
