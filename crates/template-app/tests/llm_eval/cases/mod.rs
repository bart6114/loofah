mod enhance;
mod title;
mod transcript_patch;

use hypr_template_eval::EvalCase;
use hypr_template_eval::Failed;

pub fn all(samples: usize) -> Result<Vec<EvalCase>, Failed> {
    let mut cases = vec![
        title::empty_note(samples)?,
        transcript_patch::fix_typo(samples)?,
        transcript_patch::no_change(samples)?,
        enhance::structured_summary(samples)?,
    ];
    cases.extend(enhance::personal_actions(samples)?);
    Ok(cases)
}
