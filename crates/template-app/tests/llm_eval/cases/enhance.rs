use askama::filters::{Html, escape};
use hypr_template_eval::{EvalCase, EvalMessage, Expectation, Failed, PromptFragment};
use template_app::{
    EnhanceSystem, EnhanceUser, Participant, Segment, Session, Template, Transcript, render,
};

use crate::support::render_failed;

fn summary_messages(input: EnhanceUser) -> Result<Vec<EvalMessage>, Failed> {
    Ok(vec![
        EvalMessage {
            role: "system".into(),
            content: render(Template::EnhanceSystem(EnhanceSystem {
                language: None,
                prompt_override: String::new(),
            }))
            .map_err(render_failed)?,
        },
        EvalMessage {
            role: "user".into(),
            content: render(Template::EnhanceUser(Box::new(input))).map_err(render_failed)?,
        },
    ])
}

pub fn structured_summary(samples: usize) -> Result<EvalCase, Failed> {
    Ok(EvalCase {
        name: "enhance_structured_summary".to_string(),
        messages: summary_messages(EnhanceUser {
            tag_context: Default::default(),
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
                        is_current_user: None,
                        speaker: "Alice".to_string(),
                        text: "Shipped the feature and started rollout.".to_string(),
                    },
                    Segment {
                        is_current_user: None,
                        speaker: "Bob".to_string(),
                        text: "Need to check CI before release and follow up on the PR review."
                            .to_string(),
                    },
                ],
                started_at: None,
                ended_at: None,
            }],
            pre_meeting_memo: "- align on priorities\n- review rollout risks".to_string(),
            post_meeting_memo: "- check CI\n- ship before EOD".to_string(),
        })?,
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
            Expectation::MarkdownTasks {
                count: 0,
                required_terms: vec![],
            },
            Expectation::MarkdownHasUnorderedList,
        ],
        required_pass_rate: 0.8,
        samples,
        max_tokens: 400,
        response_format: None,
    })
}

pub fn personal_actions(samples: usize) -> Result<Vec<EvalCase>, Failed> {
    let send_proposal_by_friday = r"(?:send|submit|share) (?:the )?(?:revised )?proposal by Friday";
    let update_documentation = r"(?:update|revise) (?:the )?(?:documentation|docs)";
    let bob_owns_invoice = r"(?i)\bBob\b.*\binvoice\b|\binvoice\b.*\bBob\b";

    // The illustrative score counts named ownership, explicit commitment, and a stated
    // deadline. Personal relevance is checked separately before a task can qualify.
    Ok(vec![
        personal_action_case(
            samples,
            "score_3_of_3",
            &[("Alice", true, "I'll send the proposal by Friday.")],
            Some(send_proposal_by_friday),
            &[],
            "# Action items\n- [ ] Send the proposal by Friday.",
        )?,
        personal_action_case(
            samples,
            "score_2_of_3",
            &[("Alice", true, "I'll update the documentation.")],
            Some(update_documentation),
            &[],
            "# Action items\n- [ ] Update the **documentation**.",
        )?,
        personal_action_case(
            samples,
            "score_1_of_3",
            &[("Alice", true, "Peter could investigate this.")],
            None,
            &[],
            "# Investigation\n- Alice suggested Peter could investigate.",
        )?,
        personal_action_case(
            samples,
            "score_0_of_3",
            &[("Alice", true, "We should probably look into this.")],
            None,
            &[],
            "# Discussion\n- Alice suggested looking into the issue.",
        )?,
        personal_action_case(
            samples,
            "accepted_assignment",
            &[
                (
                    "Bob",
                    false,
                    "Alice, could you send the revised proposal by Friday?",
                ),
                ("Alice", true, "Is the pricing final?"),
                ("Bob", false, "Yes, the pricing is final."),
                ("Alice", true, "Great, I'll send it."),
            ],
            Some(send_proposal_by_friday),
            &[],
            "# Proposal\n- Pricing is final.\n\n# Action items\n- [ ] Send the revised proposal by Friday.",
        )?,
        personal_action_case(
            samples,
            "unaccepted_assignment",
            &[
                (
                    "Bob",
                    false,
                    "Alice, please send the revised proposal by Friday.",
                ),
                ("Alice", true, "Is the pricing final?"),
                ("Bob", false, "Yes, the pricing is final."),
            ],
            None,
            &[],
            "# Proposal\n- Pricing is final. Bob asked Alice to send the revised proposal by Friday.",
        )?,
        personal_action_case(
            samples,
            "unstated_deadline",
            &[
                (
                    "Alice",
                    true,
                    "I'll update the documentation. I already finished the budget review.",
                ),
                ("Bob", false, "I'll send the invoice by Friday."),
            ],
            Some(update_documentation),
            &[bob_owns_invoice],
            "# Discussion\n- Alice finished the budget review.\n- Bob will send the invoice by Friday.\n\n# Action items\n- [ ] Update the documentation.",
        )?,
        personal_action_case(
            samples,
            "other_persons_commitment",
            &[
                ("Alice", true, "Who will send the invoice?"),
                ("Bob", false, "I'll send the invoice tomorrow."),
            ],
            None,
            &[bob_owns_invoice],
            "# Invoice\n- Bob will send the invoice tomorrow.",
        )?,
        personal_action_case(
            samples,
            "current_user_follow_up",
            &[
                ("Bob", false, "I'll send the invoice tomorrow."),
                (
                    "Alice",
                    true,
                    "I'll check with finance to confirm they received it.",
                ),
            ],
            Some(concat!(
                r"(?:",
                r"(?:check|follow[ -]up) with finance (?:to (?:confirm|verify) |(?:about|on) )(?:receipt of the invoice|(?:that )?(?:they|finance) received the invoice|invoice receipt)",
                r"|(?:confirm|verify) (?:with finance )?(?:receipt of the invoice|(?:that )?finance received the invoice|invoice receipt)(?: with finance)?",
                r")",
            )),
            &[bob_owns_invoice],
            "# Invoice\n- Bob will send the invoice tomorrow.\n\n# Action items\n- [ ] Check with finance to confirm receipt of the invoice.",
        )?,
        personal_action_case(
            samples,
            "unidentified_user",
            &[
                ("Alice", false, "I'll send the proposal by Friday."),
                ("Bob", false, "I'll send the invoice tomorrow."),
            ],
            None,
            &[bob_owns_invoice],
            "# Discussion\n- Alice will send the proposal by Friday.\n- Bob will send the invoice tomorrow.",
        )?,
    ])
}

fn personal_action_case(
    samples: usize,
    name: &str,
    turns: &[(&str, bool, &str)],
    task_pattern: Option<&str>,
    summary_patterns: &[&str],
    smoke_output: &str,
) -> Result<EvalCase, Failed> {
    let segments = turns
        .iter()
        .map(|&(speaker, is_current_user, text)| Segment {
            speaker: speaker.into(),
            is_current_user: Some(is_current_user),
            text: text.into(),
        })
        .collect();
    let prompt_fragments = turns
        .iter()
        .map(|&(speaker, is_current_user, text)| {
            let marker = if is_current_user {
                " [current user]"
            } else {
                ""
            };
            let text = escape(text, Html).unwrap();
            PromptFragment {
                role: "user".into(),
                needle: format!("{speaker}{marker}: {text}"),
            }
        })
        .collect();
    let mut expectations = vec![
        Expectation::NotContains("```".into()),
        Expectation::MarkdownTasks {
            count: usize::from(task_pattern.is_some()),
            required_terms: vec![],
        },
        Expectation::MarkdownAllHeadingsAreH1,
    ];
    if let Some(pattern) = task_pattern {
        expectations.push(Expectation::MarkdownHasHeadings(vec![
            "Action items".into(),
        ]));
        expectations.push(Expectation::MarkdownListItemMatches {
            checked: Some(false),
            pattern: format!(r"(?i)^(?:{pattern})[.!]?$"),
        });
    } else {
        expectations.push(Expectation::NotContains("# Action items".into()));
    }
    expectations.extend(summary_patterns.iter().map(|pattern| {
        Expectation::MarkdownListItemMatches {
            checked: None,
            pattern: (*pattern).into(),
        }
    }));

    Ok(EvalCase {
        name: format!("enhance_personal_actions_{name}"),
        messages: summary_messages(EnhanceUser {
            tag_context: Default::default(),
            session: Session {
                title: None,
                started_at: None,
                ended_at: None,
                event: None,
            },
            participants: vec![],
            transcripts: vec![Transcript {
                segments,
                started_at: None,
                ended_at: None,
            }],
            pre_meeting_memo: String::new(),
            post_meeting_memo: String::new(),
        })?,
        prompt_fragments,
        smoke_outputs: vec![smoke_output.into()],
        expectations,
        required_pass_rate: 0.8,
        samples,
        max_tokens: 400,
        response_format: None,
    })
}
