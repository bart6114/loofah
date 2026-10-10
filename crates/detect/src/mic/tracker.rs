use crate::InstalledApp;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Applications(BTreeMap<String, InstalledApp>);

impl Applications {
    pub(super) fn update(
        &mut self,
        apps: Vec<InstalledApp>,
    ) -> (Vec<InstalledApp>, Vec<InstalledApp>) {
        let current: BTreeMap<_, _> = apps.into_iter().map(|app| (app.id.clone(), app)).collect();
        let started = current
            .iter()
            .filter(|(id, _)| !self.0.contains_key(*id))
            .map(|(_, app)| app.clone())
            .collect();
        let stopped = self
            .0
            .iter()
            .filter(|(id, _)| !current.contains_key(*id))
            .map(|(_, app)| app.clone())
            .collect();
        self.0 = current;
        (started, stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> InstalledApp {
        InstalledApp {
            id: id.into(),
            name: id.into(),
        }
    }

    #[test]
    fn detects_individual_sessions_without_restarting_active_app_timers() {
        let mut tracker = Applications::default();
        assert_eq!(tracker.update(vec![app("teams"), app("zoom")]).0.len(), 2);
        assert!(
            tracker
                .update(vec![app("zoom"), app("teams"), app("zoom")])
                .0
                .is_empty()
        );
        let (started, stopped) = tracker.update(vec![app("zoom"), app("chrome")]);
        assert_eq!(started[0].id, "chrome");
        assert_eq!(stopped[0].id, "teams");
        assert_eq!(tracker.update(Vec::new()).1.len(), 2);
        assert!(tracker.update(Vec::new()).1.is_empty());
    }
}
