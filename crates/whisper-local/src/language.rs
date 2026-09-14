#[derive(Debug, Clone)]
pub struct Observation {
    pub scores: Vec<(String, f32)>,
}

impl Observation {
    fn best(&self) -> Option<(&str, f32)> {
        let total: f32 = self.scores.iter().map(|(_, p)| p.max(0.0)).sum();
        self.scores
            .iter()
            .filter(|(_, p)| p.is_finite())
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(lang, p)| (lang.as_str(), if total > 0.0 { *p / total } else { 0.0 }))
    }
}

pub struct LanguageResolver {
    selected: Option<String>,
    observations: Vec<Observation>,
    provisional: bool,
    conflict: Option<String>,
    speech_samples: usize,
    next_check: usize,
    gate: f32,
}

impl LanguageResolver {
    pub fn new(languages: &[hypr_whisper::Language]) -> Self {
        Self {
            selected: (languages.len() == 1).then(|| languages[0].to_string()),
            observations: vec![],
            provisional: false,
            conflict: None,
            speech_samples: 0,
            next_check: if languages.len() == 1 {
                usize::MAX
            } else {
                60 * 16000
            },
            gate: std::env::var("LOOFAH_WHISPER_LANGUAGE_GATE")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| [0.0, 0.5, 0.7, 0.8].contains(v))
                .unwrap_or(0.7),
        }
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn needs_observation(&self) -> bool {
        self.selected.is_none() || self.conflict.is_some() || self.speech_samples >= self.next_check
    }
    pub fn add_speech(&mut self, samples: usize) {
        self.speech_samples = self.speech_samples.saturating_add(samples);
    }

    pub fn observe(&mut self, observation: Observation) {
        let Some((winner, score)) = observation.best() else {
            return;
        };
        let winner = winner.to_owned();
        if self.selected.is_none() {
            self.observations.push(observation);
            let agreement = self.observations.len() >= 2
                && self.observations.iter().all(|o| {
                    o.best()
                        .is_some_and(|(lang, p)| lang == winner && p >= self.gate)
                });
            if agreement {
                self.selected = Some(winner);
            } else if self.observations.len() >= 3 {
                self.finish_startup();
            }
        } else {
            if self.selected.as_deref() != Some(&winner) && score >= self.gate {
                if self.conflict.as_deref() == Some(&winner) {
                    self.selected = Some(winner);
                    self.provisional = false;
                    self.conflict = None;
                } else {
                    self.conflict = Some(winner);
                }
            } else {
                self.conflict = None;
            }
            self.next_check = self.speech_samples.saturating_add(300 * 16000);
        }
        tracing::debug!(language = ?self.selected, provisional = self.provisional, observations = self.observations.len(), "whisper_language_resolution");
    }

    pub fn finish_startup(&mut self) {
        if self.selected.is_some() {
            return;
        }
        let mut votes = std::collections::BTreeMap::<String, (usize, f32)>::new();
        for observation in &self.observations {
            if let Some((winner, _)) = observation.best() {
                votes.entry(winner.to_owned()).or_default().0 += 1;
            }
            for (lang, p) in &observation.scores {
                votes.entry(lang.clone()).or_default().1 += p;
            }
        }
        self.selected = votes
            .into_iter()
            .max_by(|a, b| a.1.0.cmp(&b.1.0).then(a.1.1.total_cmp(&b.1.1)))
            .map(|(lang, _)| lang);
        self.provisional = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(en: f32, nl: f32) -> Observation {
        Observation {
            scores: vec![("en".into(), en), ("nl".into(), nl)],
        }
    }
    #[test]
    fn single_language_bypasses_detection() {
        let mut r = LanguageResolver::new(&[hypr_whisper::Language::En]);
        r.add_speech(1000 * 16000);
        assert!(!r.needs_observation());
    }
    #[test]
    fn misleading_intro_needs_third_vote() {
        let mut r = LanguageResolver::new(&[]);
        r.observe(evidence(0.9, 0.1));
        r.observe(evidence(0.1, 0.9));
        assert_eq!(r.selected(), None);
        r.observe(evidence(0.2, 0.8));
        assert_eq!(r.selected(), Some("nl"));
    }
    #[test]
    fn weak_consensus_is_bounded_and_sparse_conflict_requires_confirmation() {
        let mut r = LanguageResolver::new(&[]);
        for _ in 0..3 {
            r.observe(evidence(0.55, 0.45));
        }
        assert_eq!(r.selected(), Some("en"));
        assert!(!r.needs_observation());
        r.add_speech(60 * 16000);
        assert!(r.needs_observation());
        r.observe(evidence(0.05, 0.95));
        assert_eq!(r.selected(), Some("en"));
        assert!(r.needs_observation());
        r.observe(evidence(0.1, 0.9));
        assert_eq!(r.selected(), Some("nl"));
        r.add_speech(299 * 16000);
        assert!(!r.needs_observation());
        r.add_speech(16000);
        assert!(r.needs_observation());
    }
    #[test]
    fn deadline_uses_available_vote_and_tie_scores() {
        let mut r = LanguageResolver::new(&[]);
        r.observe(evidence(0.6, 0.4));
        r.observe(evidence(0.1, 0.9));
        r.finish_startup();
        assert_eq!(r.selected(), Some("nl"));
    }
}
