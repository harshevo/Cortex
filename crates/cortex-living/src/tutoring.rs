use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    pub question: String,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub text: String,
    pub examples: Vec<Example>,
}

impl Lesson {
    pub fn validate(&self) -> Result<(), String> {
        if self.text.trim().is_empty()
            || self.text.len() > 4096
            || self.examples.is_empty()
            || self.examples.len() > 4
        {
            return Err("Give a short lesson and 1–4 question/answer examples".into());
        }
        for example in &self.examples {
            if example.question.trim().is_empty()
                || example.answer.trim().is_empty()
                || example.question.len() > 1024
                || example.answer.len() > 2048
            {
                return Err("Teaching examples require a question and answer of at most 1,024 / 2,048 bytes".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub attempt: String,
    pub text: String,
    pub method: String,
    pub matching_score: f32,
    pub source: Option<String>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub attempt: String,
    pub correct: bool,
    pub feedback: String,
    pub correction: Option<String>,
    pub judged_by: String,
    pub verified: bool,
}
