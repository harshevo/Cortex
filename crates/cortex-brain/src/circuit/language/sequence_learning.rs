use super::*;

#[derive(Deserialize)]
struct ReplayExample {
    question: String,
    activity: Vec<f32>,
    target: Vec<usize>,
}

fn examples(value: &serde_json::Value) -> Result<Vec<ReplayExample>, String> {
    let rows: Vec<ReplayExample> =
        serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    if rows.is_empty() || rows.len() > 16 {
        return Err("Sequence replay requires 1..16 examples".into());
    }
    Ok(rows)
}

fn losses(
    decoder: &super::super::sequence::SequenceDecoder,
    dialogue: &super::super::dialogue::Dialogue,
    rows: &[ReplayExample],
) -> Result<Vec<f32>, String> {
    rows.iter()
        .map(|row| decoder.reference_loss(dialogue, &row.question, &row.activity, &row.target))
        .collect()
}

fn retained(before: &[f32], after: &[f32]) -> bool {
    before
        .iter()
        .zip(after)
        .all(|(a, b)| b.is_finite() && *b <= *a + 0.005)
        && after.iter().sum::<f32>() <= before.iter().sum::<f32>() + 0.0001 * before.len() as f32
}
fn fixed_retention(
    decoder: &super::super::sequence::SequenceDecoder,
    before: &[f32],
    after: &[f32],
) -> Result<bool, String> {
    if decoder.training["retention_reference_losses"].is_null() {
        return Ok(true);
    }
    let original: Vec<f32> =
        serde_json::from_value(decoder.training["retention_reference_losses"].clone())
            .map_err(|e| e.to_string())?;
    Ok(original.len() == before.len()
        && original.iter().all(|x| x.is_finite())
        && retained(&original, after))
}

impl Assistant {
    pub fn feedback_checkpointed(&mut self, reward: f32, path: &Path) -> Result<(), String> {
        let mut candidate = self.isolated();
        candidate.feedback(reward)?;
        candidate.save(path)?;
        candidate.network.adopt_live(&self.network);
        *self = candidate;
        Ok(())
    }
    pub(super) fn sequence_feedback(&mut self, reward: f32) -> Result<(), String> {
        let (question, activity, tokens) = self
            .last_sequence
            .as_ref()
            .ok_or("No learned answer to rate")?;
        let decoder = self.sequence_decoder.as_ref().ok_or("No learned decoder")?;
        let rows = examples(&decoder.training["retention_examples"])?;
        let dialogue = self.dialogue.as_ref().ok_or("No tokenizer")?;
        let before = losses(decoder, dialogue, &rows)?;
        let mut candidate = decoder.clone();
        let target = tokens.clone();
        candidate.train_output(dialogue, question, activity, &target, reward, 0.01)?;
        let after = losses(&candidate, dialogue, &rows)?;
        if !retained(&before, &after) || !fixed_retention(decoder, &before, &after)? {
            return Err("Feedback candidate rejected by learned decoder retention checks".into());
        }
        self.sequence_decoder = Some(candidate);
        self.last_sequence = None;
        self.adaptation.feedback_events += 1;
        self.adaptation.sequence_feedback_updates += 1;
        self.updates += 1;
        Ok(())
    }
    pub(super) fn replay_sequence(&mut self, path: &Path) -> Result<(), String> {
        let Some(decoder) = &self.sequence_decoder else {
            return Ok(());
        };
        if decoder.training["replay_examples"].is_null() {
            return Ok(());
        }
        let train = examples(&decoder.training["replay_examples"])?;
        let retention = examples(&decoder.training["retention_examples"])?;
        let dialogue = self.dialogue.as_ref().ok_or("No tokenizer")?;
        let before = losses(decoder, dialogue, &retention)?;
        let replay_before = losses(decoder, dialogue, &train)?.iter().sum::<f32>();
        let mut proposal = decoder.clone();
        for row in &train {
            proposal.train_output(
                dialogue,
                &row.question,
                &row.activity,
                &row.target,
                1.0,
                0.001,
            )?;
        }
        let after = losses(&proposal, dialogue, &retention)?;
        let replay_after = losses(&proposal, dialogue, &train)?.iter().sum::<f32>();
        let accepted = retained(&before, &after)
            && fixed_retention(decoder, &before, &after)?
            && replay_after < replay_before - 1e-7;
        let mut candidate = self.isolated();
        candidate.adaptation.sequence_replay_attempts += 1;
        if accepted {
            candidate.sequence_decoder = Some(proposal);
            candidate.adaptation.sequence_replay_accepted += 1;
        } else {
            candidate.adaptation.sequence_replay_rejected += 1;
        }
        candidate.save(path)?;
        candidate.network.adopt_live(&self.network);
        *self = candidate;
        Ok(())
    }
}
