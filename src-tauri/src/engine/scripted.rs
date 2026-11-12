//! A scripted engine for tests: returns prepared steps and records requests.

use super::{Engine, EngineEvent, StepOutcome, StepRequest};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// One scripted reply.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Returned at once.
    Reply(StepOutcome),
    /// Each piece is emitted as a `TextDelta` first, then the outcome is
    /// returned (its `text` should be the pieces joined).
    Stream(Vec<String>, StepOutcome),
    /// Emits the text as one `TextDelta` and waits until the step is
    /// cancelled (or dropped); then returns that text with no error, as a
    /// cancelled request does.
    UntilCancelled(String),
}

pub struct ScriptedEngine {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<StepRequest>>,
}

impl ScriptedEngine {
    pub fn new(steps: Vec<StepOutcome>) -> Self {
        Self::with_steps(steps.into_iter().map(Step::Reply).collect())
    }

    pub fn with_steps(steps: Vec<Step>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Adds steps after the ones still waiting.
    pub fn push(&self, steps: Vec<Step>) {
        self.steps.lock().unwrap().extend(steps);
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<StepRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Engine for ScriptedEngine {
    async fn run_step(
        &self,
        request: StepRequest,
        emit: &(dyn Fn(EngineEvent) + Send + Sync),
        cancel: CancellationToken,
    ) -> StepOutcome {
        self.requests.lock().unwrap().push(request);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("no scripted step left");
        match step {
            Step::Reply(out) => out,
            Step::Stream(pieces, out) => {
                for text in pieces {
                    emit(EngineEvent::TextDelta { text });
                }
                out
            }
            Step::UntilCancelled(text) => {
                if !text.is_empty() {
                    emit(EngineEvent::TextDelta { text: text.clone() });
                }
                cancel.cancelled().await;
                StepOutcome {
                    text,
                    ..StepOutcome::default()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolCall;

    fn request(model: &str) -> StepRequest {
        StepRequest {
            model_id: model.into(),
            messages: vec![],
            tools: None,
            session_id: "s".into(),
        }
    }

    #[tokio::test]
    async fn returns_steps_in_order_and_records_requests() {
        let first = StepOutcome {
            text: "one".into(),
            tool_calls: vec![ToolCall {
                id: "c".into(),
                name: "n".into(),
                arguments: "{}".into(),
            }],
            ..StepOutcome::default()
        };
        let second = StepOutcome {
            text: "two".into(),
            ..StepOutcome::default()
        };
        let engine = ScriptedEngine::new(vec![first.clone(), second.clone()]);
        let emitted = Mutex::new(Vec::<EngineEvent>::new());
        let emit = |e| emitted.lock().unwrap().push(e);
        let a = engine
            .run_step(request("m/a"), &emit, CancellationToken::new())
            .await;
        let b = engine
            .run_step(request("m/b"), &emit, CancellationToken::new())
            .await;
        assert_eq!((a, b), (first, second));
        let models: Vec<String> = engine.requests().into_iter().map(|r| r.model_id).collect();
        assert_eq!(models, ["m/a", "m/b"]);
        assert!(emitted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_step_can_stream_text_and_wait_for_cancel() {
        let engine = ScriptedEngine::with_steps(vec![
            Step::Stream(
                vec!["a".into(), "b".into()],
                StepOutcome {
                    text: "ab".into(),
                    ..StepOutcome::default()
                },
            ),
            Step::UntilCancelled("part".into()),
        ]);
        let emitted = Mutex::new(Vec::<EngineEvent>::new());
        let emit = |e| emitted.lock().unwrap().push(e);
        let a = engine
            .run_step(request("m"), &emit, CancellationToken::new())
            .await;
        assert_eq!(a.text, "ab");
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let (b, ()) = tokio::join!(engine.run_step(request("m"), &emit, cancel), async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            stopper.cancel();
        });
        assert_eq!(b.text, "part");
        assert_eq!(b.error, None);
        let texts: Vec<EngineEvent> = emitted.lock().unwrap().clone();
        assert_eq!(
            texts,
            ["a", "b", "part"]
                .map(|t| EngineEvent::TextDelta { text: t.into() })
                .to_vec()
        );
    }

    #[tokio::test]
    #[should_panic(expected = "no scripted step left")]
    async fn running_out_of_steps_is_a_test_bug() {
        let engine = ScriptedEngine::new(vec![]);
        engine
            .run_step(request("m"), &|_| {}, CancellationToken::new())
            .await;
    }
}
