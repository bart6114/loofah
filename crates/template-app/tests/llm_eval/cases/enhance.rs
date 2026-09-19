use hypr_template_eval::{EvalCase, EvalMessage, Expectation, Failed, PromptFragment};
use template_app::{
    EnhanceSystem, EnhanceUser, Participant, Segment, Session, Template, Transcript, render,
};

use crate::support::render_failed;

pub fn structured_summary(samples: usize) -> Result<EvalCase, Failed> {
    Ok(EvalCase {
        name: "enhance_structured_summary".to_string(),
        messages: vec![
            EvalMessage {
                role: "system".to_string(),
                content: render(Template::EnhanceSystem(EnhanceSystem {
                    language: None,
                    prompt_override: String::new(),
                }))
                .map_err(render_failed)?,
            },
            EvalMessage {
                role: "user".to_string(),
                content: render(Template::EnhanceUser(Box::new(EnhanceUser {
                    session: Session {
                        title: Some("Daily Standup".to_string()),
                        started_at: None,
                        ended_at: None,
                        event: None,
                    },
                    participants: vec![
                        Participant {
                            name: "Alice".to_string(),
                            job_title: Some("Engineer".to_string()),
                        },
                        Participant {
                            name: "Bob".to_string(),
                            job_title: Some("PM".to_string()),
                        },
                    ],
                    transcripts: vec![Transcript {
                        segments: vec![
                            Segment {
                                is_current_user: None, speaker: "Alice".to_string(),
                                text: "Shipped the feature and started rollout.".to_string(),
                            },
                            Segment {
                                is_current_user: None, speaker: "Bob".to_string(),
                                text: "Need to check CI before release and follow up on the PR review."
                                    .to_string(),
                            },
                        ],
                        started_at: None,
                        ended_at: None,
                    }],
                    pre_meeting_memo: "- align on priorities\n- review rollout risks".to_string(),
                    post_meeting_memo: "- check CI\n- ship before EOD".to_string(),
                })))
                .map_err(render_failed)?,
            },
        ],
        prompt_fragments: vec![
            PromptFragment {
                role: "system".to_string(),
                needle: "Use Markdown format without code block wrappers.".to_string(),
            },
            PromptFragment {
                role: "system".to_string(),
                needle: "Use only h1 headers.".to_string(),
            },
            PromptFragment {
                role: "user".to_string(),
                needle: "# Transcript".to_string(),
            },
            PromptFragment {
                role: "user".to_string(),
                needle: "# Pre-Meeting Notes".to_string(),
            },
            PromptFragment {
                role: "user".to_string(),
                needle: "# Meeting Notes".to_string(),
            },
        ],
        smoke_outputs: vec![
            r#"# Summary

- Alice shipped the feature and started rollout.
- Bob called out that CI must be checked before release.
- The team aligned on priorities and rollout risks.

# Follow-up discussion

- Check CI before release.
- Follow up on the PR review.
- Ship before EOD if CI passes.
"#
            .to_string(),
        ],
        expectations: vec![
            Expectation::NotContains("```".to_string()),
            Expectation::MarkdownAtLeastHeadings(2),
            Expectation::MarkdownAllHeadingsAreH1,
            Expectation::MarkdownTasks { count: 0, required_terms: vec![] },
            Expectation::MarkdownHasUnorderedList,
        ],
        required_pass_rate: 0.8,
        samples,
        max_tokens: 400,
        response_format: None,
    })
}

pub fn personal_actions(samples: usize, identified: bool) -> Result<EvalCase, Failed> {
    let mut case = structured_summary(samples)?;
    case.name = if identified {
        "enhance_personal_actions"
    } else {
        "enhance_unidentified_user"
    }
    .into();
    case.messages[1].content = render(Template::EnhanceUser(Box::new(EnhanceUser {
        session: Session { title: Some("Launch planning".into()), started_at: None, ended_at: None, event: None },
        participants: vec![], pre_meeting_memo: String::new(), post_meeting_memo: String::new(),
        transcripts: vec![Transcript { started_at: None, ended_at: None, segments: vec![
            Segment { speaker: "Alice".into(), is_current_user: Some(identified), text: "I will send the revised proposal by Friday. I already finished the budget review.".into() },
            Segment { speaker: "Bob".into(), is_current_user: Some(false), text: "I will send the invoice tomorrow. We should consider refreshing the website sometime.".into() },
        ] }],
    }))).map_err(render_failed)?;
    case.prompt_fragments = vec![PromptFragment {
        role: "user".into(),
        needle: if identified {
            "Alice [current user]:"
        } else {
            "Alice:"
        }
        .into(),
    }];
    case.expectations = vec![
        Expectation::MarkdownTasks {
            count: usize::from(identified),
            required_terms: if identified {
                vec!["proposal".into()]
            } else {
                vec![]
            },
        },
        Expectation::MarkdownAllHeadingsAreH1,
    ];
    case.smoke_outputs = vec![if identified {
        "# Discussion\n- Budget review is complete. Bob will send the invoice tomorrow. A website refresh was suggested.\n\n# Action items\n- [ ] Send the revised proposal by Friday."
    } else {
        "# Discussion\n- Alice will send the proposal by Friday and has finished the budget review. Bob will send the invoice tomorrow. A website refresh was suggested."
    }.into()];
    Ok(case)
}
