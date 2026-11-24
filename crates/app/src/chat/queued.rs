//! The images of prompts waiting in pi's queue. pi gives back (and reports) only the texts of
//! the messages it queued, so the session remembers their images, and forgets them once they
//! are delivered.

use pi_rpc::protocol::ImageContent;

/// Prompts pi queued (or may be queueing right now), oldest first.
#[derive(Default)]
pub struct QueuedPrompts {
    prompts: Vec<Queued>,
}

struct Queued {
    /// As it was sent. pi's queue may say something else: it expands skills and templates.
    text: String,
    /// `None` until pi answers that it queued the prompt.
    images: Option<Vec<ImageContent>>,
    follow_up: bool,
    listed: Listed,
    /// It left the queue before pi's answer was handled.
    gone: bool,
}

/// How pi's queue listed a prompt.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listed {
    NotYet,
    AsSent,
    /// With another text.
    Changed,
}

impl QueuedPrompts {
    /// A prompt is being sent; [`Self::accepted`] or [`Self::not_queued`] follows.
    pub fn sending(&mut self, text: &str, follow_up: bool) {
        self.prompts.push(Queued {
            text: text.to_owned(),
            images: None,
            follow_up,
            listed: Listed::NotYet,
            gone: false,
        });
    }

    /// pi queued the prompt being sent, with `images`.
    pub fn accepted(&mut self, images: Vec<ImageContent>) {
        if let Some(prompt) = self.prompts.iter_mut().find(|p| p.images.is_none()) {
            prompt.images = Some(images);
        }
        self.prompts.retain(|prompt| !prompt.gone);
    }

    /// pi did not queue the prompt being sent.
    pub fn not_queued(&mut self) {
        self.prompts.retain(|prompt| prompt.images.is_some());
    }

    /// pi's queue changed: prompts that left it were delivered.
    pub fn update(&mut self, steering: &[String], follow_up: &[String]) {
        for (texts, is_follow_up) in [(steering, false), (follow_up, true)] {
            let found = self.find(texts, is_follow_up);
            for (text, at) in texts.iter().zip(&found) {
                if let Some(at) = *at {
                    let prompt = &mut self.prompts[at];
                    prompt.listed = if prompt.text == *text {
                        Listed::AsSent
                    } else {
                        Listed::Changed
                    };
                }
            }
            for (at, prompt) in self.prompts.iter_mut().enumerate() {
                // One that was never listed is still on its way into the queue.
                prompt.gone |= prompt.follow_up == is_follow_up
                    && prompt.listed != Listed::NotYet
                    && !found.contains(&Some(at));
            }
        }
        self.prompts
            .retain(|prompt| !prompt.gone || prompt.images.is_none());
    }

    /// pi's queue was emptied and gave back these texts: the images that belong to them.
    pub fn take(&mut self, steering: &[String], follow_up: &[String]) -> Vec<ImageContent> {
        let mut images = Vec::new();
        for (texts, is_follow_up) in [(steering, false), (follow_up, true)] {
            for at in self.find(texts, is_follow_up).into_iter().flatten() {
                images.extend(self.prompts[at].images.iter_mut().flat_map(std::mem::take));
            }
        }
        self.clear();
        images
    }

    /// Nothing waits in pi's queue any more.
    pub fn clear(&mut self) {
        self.prompts.retain(|prompt| prompt.images.is_none());
    }

    /// For each of `texts` (one of pi's queues), which prompt it is. The newest match first:
    /// prompts leave the queue from its front.
    fn find(&self, texts: &[String], follow_up: bool) -> Vec<Option<usize>> {
        let mut found = vec![None; texts.len()];
        let mut free: Vec<usize> = (0..self.prompts.len())
            .filter(|at| self.prompts[*at].follow_up == follow_up && !self.prompts[*at].gone)
            .collect();
        let mut pass = |fits: &dyn Fn(&Queued, &str) -> bool| {
            for (text, found) in texts.iter().zip(&mut found).rev() {
                if found.is_none()
                    && let Some(at) = free
                        .iter()
                        .rposition(|at| fits(&self.prompts[*at], text.as_str()))
                {
                    *found = Some(free.remove(at));
                }
            }
        };
        pass(&|prompt, text| prompt.text == text && prompt.listed != Listed::NotYet);
        pass(&|prompt, text| prompt.text == text);
        // Expanded by pi, so matched by their order.
        pass(&|prompt, _| prompt.listed != Listed::AsSent);
        found
    }
}

#[cfg(test)]
mod tests {
    use pi_rpc::protocol::ImageContent;

    use super::QueuedPrompts;

    fn image(data: &str) -> ImageContent {
        ImageContent::new(data, "image/png")
    }

    fn texts(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| (*text).to_owned()).collect()
    }

    /// pi queues `text` with an image: the queue's new content, then the answer.
    fn queue(prompts: &mut QueuedPrompts, text: &str, data: &str, steering: &[&str]) {
        prompts.sending(text, false);
        prompts.update(&texts(steering), &[]);
        prompts.accepted(vec![image(data)]);
    }

    #[test]
    fn delivered_prompts_are_forgotten() {
        let mut prompts = QueuedPrompts::default();
        queue(&mut prompts, "a", "A", &["a"]);
        queue(&mut prompts, "b", "B", &["a", "b"]);
        prompts.update(&texts(&["b"]), &[]);
        assert_eq!(prompts.take(&texts(&["b"]), &[]), [image("B")]);
        assert_eq!(prompts.take(&texts(&["b"]), &[]), []);
    }

    #[test]
    fn equal_texts_and_image_only_prompts_leave_from_the_front() {
        let mut prompts = QueuedPrompts::default();
        queue(&mut prompts, "", "A", &[""]);
        queue(&mut prompts, "", "B", &["", ""]);
        prompts.update(&texts(&[""]), &[]);
        assert_eq!(prompts.take(&texts(&[""]), &[]), [image("B")]);
    }

    #[test]
    fn expanded_prompts_match_by_order() {
        let mut prompts = QueuedPrompts::default();
        queue(&mut prompts, "/skill:a", "A", &["<a>"]);
        queue(&mut prompts, "plain", "P", &["<a>", "plain"]);
        queue(&mut prompts, "/skill:b", "B", &["<a>", "plain", "<b>"]);
        let all = texts(&["<a>", "plain", "<b>"]);
        prompts.update(&all, &[]);
        prompts.update(&all[1..], &[]);
        assert_eq!(prompts.take(&all[1..], &[]), [image("P"), image("B")]);
    }

    #[test]
    fn another_queues_prompts_are_not_mixed_in() {
        let mut prompts = QueuedPrompts::default();
        prompts.sending("later", true);
        prompts.update(&[], &texts(&["later"]));
        prompts.accepted(vec![image("L")]);
        queue(&mut prompts, "now", "N", &["now"]);
        // pi reports both queues each time; "later" was delivered.
        assert_eq!(prompts.take(&texts(&["now"]), &[]), [image("N")]);
    }

    #[test]
    fn answers_and_queue_updates_come_in_any_order() {
        let mut prompts = QueuedPrompts::default();
        // The answer before the update that lists it, with an unrelated one between.
        prompts.sending("a", false);
        prompts.accepted(vec![image("A")]);
        prompts.update(&[], &[]);
        prompts.update(&texts(&["a"]), &[]);
        // Delivered before its answer was handled.
        prompts.sending("b", false);
        prompts.update(&texts(&["a", "b"]), &[]);
        prompts.update(&[], &[]);
        prompts.accepted(vec![image("B")]);
        assert_eq!(prompts.take(&texts(&["b"]), &[]), []);
        // Started a run instead of joining the queue.
        prompts.sending("c", false);
        prompts.not_queued();
        assert_eq!(prompts.take(&texts(&["c"]), &[]), []);
    }
}
