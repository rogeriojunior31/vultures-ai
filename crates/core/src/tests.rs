use super::*;

/// `reduce` without its audit lines: the tests below are about what the app does. Every call still
/// checks that each answer came with exactly its audit line, so all of them guard the audit log.
fn visible(state: &mut State, input: Input, now: Instant) -> Vec<Effect> {
    let effects = reduce(state, input, now);
    let answers = effects
        .iter()
        .filter(|e| {
            matches!(
                e,
                Effect::RespondPermission { .. } | Effect::AnswerQuestion { .. }
            )
        })
        .count();
    let lines = effects
        .iter()
        .filter(|e| {
            matches!(
                e,
                Effect::Audit(a) if matches!(
                    a.act,
                    audit::Act::Allow | audit::Act::Deny | audit::Act::AlwaysAllow | audit::Act::Answer
                )
            )
        })
        .count();
    assert_eq!(answers, lines, "an answer without its audit line: {effects:?}");
    effects
        .into_iter()
        .filter(|e| !matches!(e, Effect::Audit(_)))
        .collect()
}

fn key(id: &str) -> SessionKey {
    SessionKey {
        agent: AgentKind::Claude,
        session_id: id.into(),
    }
}

fn agent(session: &str, event: AgentEvent) -> Input {
    Input::Agent(AgentUpdate {
        session: key(session),
        cwd: Some("/home/me/vults".into()),
        terminal: Terminal {
            pid: Some(42),
            ..Default::default()
        },
        agent_id: None,
        event,
    })
}

/// The same, from one of the session's subagents.
fn from_subagent(session: &str, sub: &str, event: AgentEvent) -> Input {
    let Input::Agent(mut u) = agent(session, event) else {
        unreachable!()
    };
    u.agent_id = Some(sub.into());
    Input::Agent(u)
}

fn requested(session: &str, id: &str) -> Input {
    agent(
        session,
        AgentEvent::PermissionRequested {
            request: RequestId(id.into()),
            tool: "Bash".into(),
            target: "Bash · cargo test".into(),
            ask: Ask::default(),
        },
    )
}

fn decide(id: &str, decision: Decision) -> Input {
    Input::User(Intent::Decide {
        request: RequestId(id.into()),
        decision,
    })
}

fn rid(id: &str) -> RequestId {
    RequestId(id.into())
}

#[test]
fn a_request_is_acked_and_answered_once() {
    let mut s = State::default();
    let now = Instant::now();
    assert_eq!(
        visible(&mut s, requested("a", "r1"), now),
        vec![Effect::AckPermission(rid("r1"))]
    );
    assert_eq!(s.sessions[&key("a")].status, Status::Approval);
    assert_eq!(
        visible(&mut s, decide("r1", Decision::Allow), now),
        vec![Effect::RespondPermission {
            request: rid("r1"),
            decision: Decision::Allow
        }]
    );
    assert_eq!(s.sessions[&key("a")].status, Status::Working);
    assert!(visible(&mut s, decide("r1", Decision::Deny), now).is_empty());
}

#[test]
fn unknown_or_stale_requests_are_never_answered() {
    let mut s = State::default();
    let now = Instant::now();
    assert!(visible(&mut s, decide("ghost", Decision::Allow), now).is_empty());
    visible(&mut s, requested("a", "r1"), now);
    assert!(visible(&mut s, decide("other", Decision::Allow), now).is_empty());
    assert!(
        !s.pending.is_empty(),
        "a click on another id must not drop the real card"
    );
}

#[test]
fn a_second_request_waits_its_turn() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    assert_eq!(
        visible(&mut s, requested("b", "r2"), now),
        vec![Effect::AckPermission(rid("r2"))]
    );
    let view = s.view().approval.unwrap();
    assert_eq!((view.request.as_str(), view.queue), ("r1", 2));
    assert_eq!(s.sessions[&key("b")].status, Status::Approval);

    visible(&mut s, decide("r1", Decision::Allow), now);
    assert_eq!(s.sessions[&key("a")].status, Status::Working);
    let view = s.view().approval.unwrap();
    assert_eq!(
        (view.request.as_str(), view.session.as_str(), view.queue),
        ("r2", "b", 1)
    );
    // The one behind can be answered too, but only by its own id.
    assert!(visible(&mut s, decide("r1", Decision::Allow), now).is_empty());
    visible(&mut s, decide("r2", Decision::Deny), now);
    assert!(s.pending.is_empty());
}

#[test]
fn every_waiting_card_expires_on_its_own_deadline() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, requested("b", "r2"), now + PENDING_TTL / 2);
    assert_eq!(
        visible(&mut s, Input::Tick, now + PENDING_TTL),
        vec![Effect::ReleasePermission(rid("r1"))]
    );
    assert_eq!(s.view().approval.map(|a| a.request), Some("r2".into()));
    assert_eq!(
        visible(&mut s, Input::Tick, now + PENDING_TTL / 2 + PENDING_TTL),
        vec![Effect::ReleasePermission(rid("r2"))]
    );
}

#[test]
fn a_subagent_working_leaves_the_main_agents_card_alone() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    let step = AgentEvent::ToolStarted(Step {
        activity: Activity::Read,
        tool: "Read".into(),
        detail: None,
    });
    assert!(visible(&mut s, from_subagent("a", "sub-1", step.clone()), now).is_empty());
    assert_eq!(s.pending.len(), 1);
    // The main agent itself moving on means the user answered in the terminal.
    assert_eq!(
        visible(&mut s, agent("a", step), now),
        vec![Effect::ReleasePermission(rid("r1"))]
    );
}

#[test]
fn always_also_answers_the_same_request_waiting_again() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, from_subagent("a", "sub-1", requested_event("r2")), now);
    let effects = visible(
        &mut s,
        Input::User(Intent::DecideAlways { request: rid("r1") }),
        now,
    );
    assert_eq!(
        &effects[..2],
        &[
            Effect::RespondPermission {
                request: rid("r1"),
                decision: Decision::Allow
            },
            Effect::RespondPermission {
                request: rid("r2"),
                decision: Decision::Allow
            },
        ]
    );
    assert!(matches!(effects[2], Effect::SaveRules(_)));
    assert!(s.pending.is_empty());
    assert_eq!(s.sessions[&key("a")].status, Status::Working);
}

#[test]
fn a_step_a_rule_allowed_says_so() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(
        &mut s,
        Input::User(Intent::DecideAlways { request: rid("r1") }),
        now,
    );
    visible(
        &mut s,
        agent(
            "a",
            AgentEvent::ToolStarted(Step {
                activity: Activity::Run,
                tool: "Bash".into(),
                detail: Some("cargo test".into()),
            }),
        ),
        now,
    );
    visible(&mut s, requested("a", "r2"), now);
    assert!(s.pending.is_empty(), "the rule answered it");
    assert_eq!(
        s.view().sessions[0].step.as_deref(),
        Some("Testing cargo test · always allowed")
    );
}

fn requested_event(id: &str) -> AgentEvent {
    AgentEvent::PermissionRequested {
        request: RequestId(id.into()),
        tool: "Bash".into(),
        target: "Bash · cargo test".into(),
        ask: Ask::default(),
    }
}

#[test]
fn the_card_expires_with_the_hook() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    assert!(visible(&mut s, Input::Tick, now + PENDING_TTL / 2).is_empty());
    assert_eq!(
        visible(&mut s, Input::Tick, now + PENDING_TTL),
        vec![Effect::ReleasePermission(rid("r1"))]
    );
    assert!(s.pending.is_empty());
}

#[test]
fn the_session_moving_on_releases_its_card() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    // Another session's events leave the card alone.
    assert!(visible(&mut s, agent("b", AgentEvent::PromptSubmitted), now).is_empty());
    assert_eq!(
        visible(
            &mut s,
            agent(
                "a",
                AgentEvent::ToolFinished {
                    failed: false,
                    target: None,
                    diff: None,
                }
            ),
            now
        ),
        vec![Effect::ReleasePermission(rid("r1"))]
    );
}

#[test]
fn a_finished_call_leaves_a_parallel_card_waiting() {
    let mut s = State::default();
    let now = Instant::now();
    let ask = |id: &str, target: &str| {
        agent(
            "a",
            AgentEvent::PermissionRequested {
                request: rid(id),
                tool: "WebFetch".into(),
                target: target.into(),
                ask: Ask::default(),
            },
        )
    };
    visible(&mut s, ask("r1", "WebFetch · a.dev"), now);
    visible(&mut s, ask("r2", "WebFetch · b.dev"), now);
    visible(&mut s, decide("r1", Decision::Allow), now);
    let done = |target: &str| {
        agent(
            "a",
            AgentEvent::ToolFinished {
                failed: false,
                target: Some(target.into()),
                diff: None,
            },
        )
    };
    // The allowed call ran: the other card still waits for the user, and stays on screen (the
    // island shows the card only while the session says it needs approval).
    assert!(visible(&mut s, done("WebFetch · a.dev"), now).is_empty());
    assert_eq!(s.pending.len(), 1);
    let view = s.view();
    assert_eq!(view.sessions[0].status, Status::Approval);
    assert_eq!(view.approval.map(|a| a.request), Some("r2".to_string()));
    // The waiting call finishing means the user answered it in the terminal.
    assert_eq!(
        visible(&mut s, done("WebFetch · b.dev"), now),
        vec![Effect::ReleasePermission(rid("r2"))]
    );
}

fn asked(session: &str, id: &str) -> Input {
    let choice = |label: &str| Choice {
        label: label.into(),
        description: None,
    };
    agent(
        session,
        AgentEvent::QuestionAsked {
            request: rid(id),
            target: "AskUserQuestion".into(),
            questions: vec![
                Question {
                    question: "Which color?".into(),
                    header: "Color".into(),
                    options: vec![choice("Red"), choice("Blue")],
                    multi: false,
                },
                Question {
                    question: "Which sizes?".into(),
                    header: "Sizes".into(),
                    options: vec![choice("S"), choice("M")],
                    multi: true,
                },
            ],
        },
    )
}

fn answer(id: &str, answers: Vec<Answer>) -> Input {
    Input::User(Intent::Answer {
        request: rid(id),
        answers,
    })
}

fn one(t: &str) -> Answer {
    Answer::One(t.into())
}

#[test]
fn a_question_card_is_answered_only_with_one_reply_per_question() {
    let mut s = State::default();
    let now = Instant::now();
    assert_eq!(
        visible(&mut s, asked("a", "q1"), now),
        vec![Effect::AckPermission(rid("q1"))]
    );
    assert_eq!(s.sessions[&key("a")].status, Status::Question);
    assert_eq!(s.sessions[&key("a")].note.as_deref(), Some("Which color?"));
    let view = s.view();
    assert_eq!(view.approval.as_ref().unwrap().questions.len(), 2);

    // Allow, Always, a missing reply, a blank one, or several where one is asked: nothing.
    assert!(visible(&mut s, decide("q1", Decision::Allow), now).is_empty());
    assert!(visible(&mut s, always("q1"), now).is_empty());
    assert!(visible(&mut s, answer("q1", vec![one("Blue")]), now).is_empty());
    assert!(visible(&mut s, answer("q1", vec![one(" "), one("S")]), now).is_empty());
    let many = Answer::Many(vec!["Red".into(), "Blue".into()]);
    assert!(visible(&mut s, answer("q1", vec![many, one("S")]), now).is_empty());
    assert_eq!(s.pending.len(), 1);

    // The user's own words count as a reply.
    let replies = vec![one("Teal, please"), Answer::Many(vec!["S".into(), "M".into()])];
    assert_eq!(
        visible(&mut s, answer("q1", replies.clone()), now),
        vec![Effect::AnswerQuestion {
            request: rid("q1"),
            answers: replies.clone()
        }]
    );
    assert_eq!(s.sessions[&key("a")].status, Status::Working);
    assert!(visible(&mut s, answer("q1", replies), now).is_empty());
}

#[test]
fn a_question_card_can_go_back_to_the_terminal() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, asked("a", "q1"), now);
    assert_eq!(
        visible(&mut s, Input::User(Intent::Release { request: rid("q1") }), now),
        vec![Effect::ReleasePermission(rid("q1"))]
    );
    assert!(s.pending.is_empty());
    // A permission can't be answered as a question.
    visible(&mut s, requested("a", "r1"), now);
    assert!(visible(&mut s, answer("r1", vec![one("yes")]), now).is_empty());
}

/// Where a quiet intent (one that must never answer a card) sits in the test's coverage list;
/// `None` for the three that may answer (ADR 0004). Exhaustive on purpose: a new `Intent` stops
/// the build here until someone decides which it is, and a quiet one needs a sample below.
fn quiet_index(intent: &Intent) -> Option<usize> {
    match intent {
        Intent::Decide { .. } | Intent::DecideAlways { .. } | Intent::Answer { .. } => None,
        Intent::Release { .. } => Some(0),
        Intent::OpenAlert { .. } => Some(1),
        Intent::Jump { .. } => Some(2),
        Intent::DismissAlert { .. } => Some(3),
        Intent::OpenRow { .. } => Some(4),
        Intent::Focus { .. } => Some(5),
        Intent::FocusNext => Some(6),
        Intent::FocusPrevious => Some(7),
        Intent::OpenFolder { .. } => Some(8),
        Intent::OpenFile { .. } => Some(9),
        Intent::SetProjectPref { .. } => Some(10),
        Intent::Hush { .. } => Some(11),
        Intent::DismissDigest => Some(12),
        Intent::SetProjectBird { .. } => Some(13),
        Intent::DismissRecap => Some(14),
    }
}

const QUIET_INTENTS: usize = 15;

/// Every quiet intent, aimed at the waiting permission, the waiting question, and things gone.
fn quiet_intents() -> Vec<Intent> {
    let mut intents = Vec::new();
    for id in ["r1", "q1", "gone"] {
        intents.push(Intent::Release { request: rid(id) });
    }
    for session in ["a", "b", "gone"] {
        intents.push(Intent::Jump {
            session: key(session),
        });
        intents.push(Intent::Focus {
            session: Some(key(session)),
        });
        intents.push(Intent::OpenFolder {
            session: key(session),
        });
        intents.push(Intent::OpenFile {
            session: key(session),
            step: 1,
            file: 0,
        });
        for hush in [
            silence::Hush::Snooze,
            silence::Hush::KeepGoing,
            silence::Hush::Dismiss,
        ] {
            intents.push(Intent::Hush {
                session: key(session),
                hush,
            });
        }
        for pref in [ProjectPref::Mute, ProjectPref::Pin, ProjectPref::Hide] {
            for on in [true, false] {
                intents.push(Intent::SetProjectPref {
                    session: key(session),
                    pref,
                    on,
                });
            }
        }
        for species in [Some("vultur"), Some("papa"), None] {
            intents.push(Intent::SetProjectBird {
                session: key(session),
                species: species.map(str::to_string),
            });
        }
    }
    intents.push(Intent::Focus { session: None });
    intents.push(Intent::DismissDigest);
    intents.push(Intent::DismissRecap);
    intents.push(Intent::FocusNext);
    intents.push(Intent::FocusPrevious);
    for k in ["k1", "gone"] {
        intents.push(Intent::OpenAlert { key: k.into() });
        intents.push(Intent::DismissAlert { key: k.into() });
    }
    for item in ["i1", "gone"] {
        intents.push(Intent::OpenRow {
            connector: "github".into(),
            item: item.into(),
        });
    }
    intents
}

/// Rule 2 (ADRs 0004 and 0014): a card is answered only by a click (`Decide`, `DecideAlways`) or an
/// exact *Always* rule. A policy will answer too once the user accepted it and the audit log
/// exists (plan-zeca W2); until then no other input may, and a policy joins this test the day it
/// lands.
#[test]
fn only_decide_can_respond() {
    // Every other input, two at a time, with a permission and a question waiting: none answers
    // either card.
    let events = [
        AgentEvent::SessionStarted,
        AgentEvent::PromptSubmitted,
        AgentEvent::ToolStarted(Step {
            activity: Activity::Run,
            tool: "Bash".into(),
            detail: None,
        }),
        AgentEvent::ToolFinished {
            failed: true,
            target: None,
            diff: None,
        },
        AgentEvent::Question { message: "?".into() },
        AgentEvent::RateLimited,
        AgentEvent::Stopped { message: None },
        AgentEvent::StopFailed { error: None },
        AgentEvent::SubagentStarted,
        AgentEvent::SubagentStopped,
        AgentEvent::SessionEnded,
    ];
    let intents = quiet_intents();
    let mut covered = [false; QUIET_INTENTS];
    for intent in &intents {
        let i = quiet_index(intent).unwrap_or_else(|| panic!("{intent:?} may answer a card"));
        covered[i] = true;
    }
    assert!(covered.iter().all(|c| *c), "every quiet intent has a sample");

    // A deciding intent aimed at the other kind of card, or at one that is gone, answers nothing.
    let misaimed = [
        decide("q1", Decision::Allow),
        decide("gone", Decision::Allow),
        always("q1"),
        always("gone"),
        answer("r1", vec![]),
        answer("r1", vec![one("yes")]),
        answer("gone", vec![one("Red"), one("S")]),
    ];
    // A rule saved later answers the next request, never a card already waiting.
    let rule = Rule {
        agent: AgentKind::Claude,
        cwd: "/home/me/vults".into(),
        tool: "Bash".into(),
        target: "Bash · cargo test".into(),
    };
    let others = [
        Input::SetRules(vec![rule]),
        Input::SetRules(Vec::new()),
        Input::SetFlock(flock::Flock::World),
        Input::SetOutfit(looks::Outfit::WitchHat),
        Input::Today(looks::Date::new(2026, 10, 31)),
        Input::SetPresence(Presence::Island),
        Input::SetPresence(Presence::Panel),
        Input::SetPresence(Presence::Quiet),
        Input::SetPresence(Presence::Paused),
        Input::SetDnd(Some(Instant::now() + Duration::from_secs(3600))),
        Input::SetDnd(None),
        Input::Locked {
            locked: true,
            missed: Vec::new(),
        },
        Input::Locked {
            locked: false,
            missed: vec![key("a"), key("b")],
        },
        Input::SetProject {
            cwd: "/home/me/vults".into(),
            prefs: ProjectPrefs {
                mute: true,
                pin: true,
                hide: true,
                species: Some("vultur".into()),
            },
        },
        alert("k2", "https://github.com/me/app/pull/13"),
        card(Vec::new()),
    ];
    let inputs: Vec<Input> = events
        .iter()
        .flat_map(|e| [agent("a", e.clone()), agent("b", e.clone())])
        .chain(intents.into_iter().map(Input::User))
        .chain(misaimed)
        .chain(others)
        .collect();
    for first in &inputs {
        for second in &inputs {
            let mut s = State::default();
            let now = Instant::now();
            let mut effects = visible(&mut s, requested("a", "r1"), now);
            effects.extend(visible(&mut s, asked("b", "q1"), now));
            visible(&mut s, alert("k1", "https://github.com/me/app/pull/12"), now);
            visible(
                &mut s,
                card(vec![row("i1", "https://github.com/me/app/pull/12")]),
                now,
            );
            effects.extend(visible(&mut s, first.clone(), now));
            effects.extend(visible(&mut s, Input::Tick, now));
            effects.extend(visible(&mut s, second.clone(), now));
            effects.extend(visible(&mut s, Input::Tick, now + PENDING_TTL));
            assert!(
                !effects.iter().any(|e| matches!(
                    e,
                    Effect::RespondPermission { .. } | Effect::AnswerQuestion { .. }
                )),
                "{first:?} then {second:?} answered a card"
            );
            // Nor does any of them say a card was answered here: an outcome only tells.
            assert!(
                s.ended.iter().all(|e| matches!(
                    e.outcome,
                    Outcome::Released | Outcome::Terminal | Outcome::Expired
                )),
                "{first:?} then {second:?} ended a card as answered"
            );
        }
    }
}

#[test]
fn steps_and_subagents() {
    let mut s = State::default();
    let now = Instant::now();
    for i in 0..10 {
        let step = Step {
            activity: Activity::Read,
            tool: "Read".into(),
            detail: Some(format!("f{i}.rs")),
        };
        visible(&mut s, agent("a", AgentEvent::ToolStarted(step)), now);
    }
    visible(&mut s, agent("a", AgentEvent::SubagentStarted), now);
    visible(&mut s, agent("a", AgentEvent::SubagentStopped), now);
    visible(&mut s, agent("a", AgentEvent::SubagentStopped), now);
    let session = &s.sessions[&key("a")];
    assert_eq!(session.steps.len(), MAX_STEPS);
    assert_eq!(session.step_count, 10);
    let view = s.view();
    assert_eq!(view.sessions[0].steps.len(), MAX_STEPS);
    assert_eq!(
        view.sessions[0].steps.last().map(String::as_str),
        Some("Reading f9.rs")
    );
    assert_eq!(
        session.steps.back().and_then(|s| s.detail.as_deref()),
        Some("f9.rs")
    );
    assert_eq!(session.subagents, 0);
    assert_eq!(session.project, "vults");
    assert_eq!(session.activity, Some(Activity::Read));
}

#[test]
fn ending_a_session_forgets_it() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::SessionStarted), now);
    visible(&mut s, agent("a", AgentEvent::SessionEnded), now);
    assert!(s.sessions.is_empty());
}

#[test]
fn project_names() {
    assert_eq!(project_name("/home/me/app/").as_deref(), Some("app"));
    assert_eq!(project_name(r"C:\work\site").as_deref(), Some("site"));
    assert_eq!(project_name("/"), None);
}

#[test]
fn the_view_shows_the_card_and_labels() {
    let mut s = State::default();
    let now = Instant::now();
    let step = Step {
        activity: Activity::Edit,
        tool: "Edit".into(),
        detail: Some("main.rs".into()),
    };
    visible(&mut s, agent("a", AgentEvent::ToolStarted(step)), now);
    visible(&mut s, requested("a", "r1"), now);
    let view = s.view();
    assert_eq!(view.sessions.len(), 1);
    assert_eq!(view.sessions[0].step.as_deref(), Some("Editing main.rs"));
    let approval = view.approval.unwrap();
    assert_eq!(approval.request, "r1");
    assert_eq!(approval.target, "Bash · cargo test");
}

fn alert(key: &str, url: &str) -> Input {
    news(key, None, url)
}

fn news(key: &str, topic: Option<&str>, url: &str) -> Input {
    Input::Connector(Alert {
        key: key.into(),
        topic: topic.map(Into::into),
        seq: 0,
        connector: "github".into(),
        level: AlertLevel::Error,
        title: "Checks failed · me/app#12".into(),
        detail: "Add the flock".into(),
        url: SafeUrl::parse(url),
    })
}

#[test]
fn alerts_are_kept_newest_first_and_capped() {
    let mut s = State::default();
    let now = Instant::now();
    for i in 0..7 {
        visible(
            &mut s,
            alert(&format!("k{i}"), "https://github.com/me/app/pull/12"),
            now,
        );
    }
    // The same key replaces, it does not pile up.
    visible(&mut s, alert("k6", "https://github.com/me/app/pull/12"), now);
    let keys: Vec<_> = s.view().alerts.into_iter().map(|a| a.key).collect();
    assert_eq!(keys, ["k6", "k5", "k4", "k3", "k2"]);
}

#[test]
fn a_newer_alert_of_the_same_story_retires_the_older() {
    let mut s = State::default();
    let now = Instant::now();
    let pr = "https://github.com/me/app/pull/12";
    let ci = Some("pr:me/app#12:ci");
    visible(&mut s, news("pr:me/app#12:ci-failed:p1", ci, pr), now);
    visible(
        &mut s,
        news("pr:me/app#12:approved", Some("pr:me/app#12:review"), pr),
        now,
    );
    visible(
        &mut s,
        news("branch:me/app:ci-failed:b1", Some("branch:me/app:ci"), pr),
        now,
    );
    visible(&mut s, news("pr:me/app#12:ci-passed:p1", ci, pr), now);
    let keys = |s: &State| s.view().alerts.into_iter().map(|a| a.key).collect::<Vec<_>>();
    assert_eq!(
        keys(&s),
        [
            "pr:me/app#12:ci-passed:p1",
            "branch:me/app:ci-failed:b1",
            "pr:me/app#12:approved"
        ],
        "fail then pass on one pull request leaves one alert; other stories stay"
    );
    // A failure on a newer commit retires the pass too.
    visible(&mut s, news("pr:me/app#12:ci-failed:p2", ci, pr), now);
    assert_eq!(keys(&s)[0], "pr:me/app#12:ci-failed:p2");
    assert_eq!(s.alerts.iter().filter(|a| a.topic.as_deref() == ci).count(), 1);
}

#[test]
fn a_re_requested_review_alerts_again() {
    let mut s = State::default();
    let now = Instant::now();
    let requested = || {
        alert(
            "review:team/lib#7:requested",
            "https://github.com/team/lib/pull/7",
        )
    };
    visible(&mut s, requested(), now);
    let first = s.view().alerts[0].seq;
    // Still on screen when it is requested again: one alert, but news again.
    visible(&mut s, requested(), now);
    let view = s.view();
    assert_eq!(view.alerts.len(), 1);
    assert!(view.alerts[0].seq > first);
    // Dismissed, then requested again.
    let dismiss = Intent::DismissAlert {
        key: "review:team/lib#7:requested".into(),
    };
    visible(&mut s, Input::User(dismiss), now);
    visible(&mut s, requested(), now);
    assert!(s.view().alerts[0].seq > view.alerts[0].seq);
}

#[test]
fn opening_an_alert_opens_only_a_safe_link() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, alert("good", "https://github.com/me/app/pull/12"), now);
    visible(&mut s, alert("bad", "https://github.com.evil.example/x"), now);
    assert!(!s.view().alerts.iter().find(|a| a.key == "bad").unwrap().link);
    let open = |k: &str| Input::User(Intent::OpenAlert { key: k.into() });
    assert_eq!(
        visible(&mut s, open("good"), now),
        vec![Effect::OpenUrl(
            SafeUrl::parse("https://github.com/me/app/pull/12").unwrap()
        )]
    );
    assert!(visible(&mut s, open("bad"), now).is_empty());
    assert!(s.alerts.is_empty(), "opened alerts are done");
}

fn row(item: &str, url: &str) -> board::Row {
    board::Row {
        item: item.into(),
        group: board::Group::Yours,
        name: "app#12".into(),
        title: "Add the flock".into(),
        checks: Some(board::Checks::Failing),
        review: None,
        url: SafeUrl::parse(url),
    }
}

fn card(rows: Vec<board::Row>) -> Input {
    Input::Board {
        connector: "github".into(),
        rows: Some(rows),
    }
}

#[test]
fn a_card_shows_until_its_connector_is_switched_off() {
    let mut s = State::default();
    let now = Instant::now();
    assert!(s.view().boards.is_empty(), "no card before the first poll");
    visible(
        &mut s,
        card(vec![row("pr:me/app#12", "https://github.com/me/app/pull/12")]),
        now,
    );
    let view = s.view();
    assert_eq!(view.boards.len(), 1);
    assert_eq!(view.boards[0].connector, "github");
    assert_eq!(view.boards[0].rows[0].item, "pr:me/app#12");
    assert!(view.boards[0].rows[0].link);
    // Nothing open is still a card: it says so.
    visible(&mut s, card(vec![]), now);
    assert!(s.view().boards[0].rows.is_empty());
    visible(
        &mut s,
        Input::Board {
            connector: "github".into(),
            rows: None,
        },
        now,
    );
    assert!(s.view().boards.is_empty());
}

#[test]
fn opening_a_row_opens_only_a_safe_link() {
    let mut s = State::default();
    let now = Instant::now();
    visible(
        &mut s,
        card(vec![
            row("pr:me/app#12", "https://github.com/me/app/pull/12"),
            row("pr:me/app#13", "https://github.com.evil.example/x"),
        ]),
        now,
    );
    assert!(!s.view().boards[0].rows[1].link);
    let open = |connector: &str, item: &str| {
        Input::User(Intent::OpenRow {
            connector: connector.into(),
            item: item.into(),
        })
    };
    assert_eq!(
        visible(&mut s, open("github", "pr:me/app#12"), now),
        vec![Effect::OpenUrl(
            SafeUrl::parse("https://github.com/me/app/pull/12").unwrap()
        )]
    );
    assert!(visible(&mut s, open("github", "pr:me/app#13"), now).is_empty());
    assert!(visible(&mut s, open("github", "pr:me/app#99"), now).is_empty());
    assert!(visible(&mut s, open("other", "pr:me/app#12"), now).is_empty());
    assert_eq!(s.view().boards[0].rows.len(), 2, "a row stays after it is opened");
}

#[test]
fn a_pull_request_that_left_the_card_takes_its_alerts() {
    let mut s = State::default();
    let now = Instant::now();
    let pr = "https://github.com/me/app/pull/12";
    visible(
        &mut s,
        card(vec![
            row("pr:me/app#12", pr),
            row("pr:me/app#1", pr),
            row("review:team/lib#7", pr),
        ]),
        now,
    );
    visible(
        &mut s,
        news("pr:me/app#12:ci-failed:p1", Some("pr:me/app#12:ci"), pr),
        now,
    );
    visible(&mut s, alert("pr:me/app#12:approved", pr), now);
    visible(&mut s, alert("pr:me/app#1:changes", pr), now);
    visible(&mut s, alert("review:team/lib#7:requested", pr), now);
    let mut elsewhere = news("pr:me/app#12:ci-failed:x", None, pr);
    if let Input::Connector(a) = &mut elsewhere {
        a.connector = "other".into();
    }
    visible(&mut s, elsewhere, now);
    // Merged: #12 is gone. `#1` is not a prefix match of `#12`, nor the other way round.
    visible(
        &mut s,
        card(vec![row("pr:me/app#1", pr), row("review:team/lib#7", pr)]),
        now,
    );
    let keys: Vec<_> = s.view().alerts.into_iter().map(|a| a.key).collect();
    assert_eq!(
        keys,
        [
            "pr:me/app#12:ci-failed:x",
            "review:team/lib#7:requested",
            "pr:me/app#1:changes"
        ],
        "another connector's alert under the same key stays"
    );
    // The review request withdrawn, then the connector switched off: switching off keeps alerts.
    visible(&mut s, card(vec![row("pr:me/app#1", pr)]), now);
    visible(
        &mut s,
        Input::Board {
            connector: "github".into(),
            rows: None,
        },
        now,
    );
    let keys: Vec<_> = s.view().alerts.into_iter().map(|a| a.key).collect();
    assert_eq!(keys, ["pr:me/app#12:ci-failed:x", "pr:me/app#1:changes"]);
}

#[test]
fn a_branch_without_checks_keeps_its_alerts_off_the_card() {
    let mut s = State::default();
    let now = Instant::now();
    let url = "https://github.com/me/app/commit/b2";
    let branch = |checks| board::Row {
        group: board::Group::Branches,
        checks,
        ..row("branch:me/app", url)
    };
    visible(&mut s, card(vec![branch(Some(board::Checks::Failing))]), now);
    visible(&mut s, alert("branch:me/app:ci-failed:b1", url), now);
    // A `[skip ci]` push: no checks on the new head.
    visible(&mut s, card(vec![branch(None)]), now);
    assert_eq!(s.view().alerts.len(), 1, "the failure was not fixed");
    assert!(
        s.view().boards[0].rows.is_empty(),
        "but the card has nothing to say about it"
    );
}

#[test]
fn silent_sessions_leave_the_wire() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("busy", AgentEvent::PromptSubmitted), now);
    visible(&mut s, agent("done", AgentEvent::Stopped { message: None }), now);
    visible(&mut s, requested("asking", "r1"), now);

    visible(&mut s, Input::Tick, now + FINISHED_TTL);
    assert!(
        !s.sessions.contains_key(&key("done")),
        "a finished session leaves after a while"
    );
    assert!(s.sessions.contains_key(&key("busy")));

    // A newer event keeps a session alive.
    visible(
        &mut s,
        agent("busy", AgentEvent::PromptSubmitted),
        now + SESSION_TTL / 2,
    );
    visible(&mut s, Input::Tick, now + SESSION_TTL);
    assert!(s.sessions.contains_key(&key("busy")));
    assert!(s.pending.is_empty(), "the card expired with its hook long ago");
    visible(&mut s, Input::Tick, now + SESSION_TTL * 2);
    assert!(s.sessions.is_empty());
}

#[test]
fn a_click_on_a_session_jumps_to_its_terminal() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    assert_eq!(
        visible(&mut s, Input::User(Intent::Jump { session: key("a") }), now),
        vec![Effect::JumpToTerminal(Terminal {
            pid: Some(42),
            ..Default::default()
        })]
    );
    assert!(visible(&mut s, Input::User(Intent::Jump { session: key("gone") }), now).is_empty());
}

fn always(id: &str) -> Input {
    Input::User(Intent::DecideAlways {
        request: RequestId(id.into()),
    })
}

#[test]
fn always_allows_that_exact_thing_in_that_project_only() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    let effects = visible(&mut s, always("r1"), now);
    assert_eq!(
        effects[0],
        Effect::RespondPermission {
            request: rid("r1"),
            decision: Decision::Allow
        }
    );
    assert!(matches!(&effects[1], Effect::SaveRules(r) if r.len() == 1));

    // The same command in the same project: answered at once, no card.
    assert_eq!(
        visible(&mut s, requested("a", "r2"), now),
        vec![
            Effect::AckPermission(rid("r2")),
            Effect::RespondPermission {
                request: rid("r2"),
                decision: Decision::Allow
            }
        ]
    );
    assert!(s.pending.is_empty());

    // Another command: a card as usual.
    let other = Input::Agent(AgentUpdate {
        session: key("a"),
        cwd: Some("/home/me/vults".into()),
        terminal: Terminal::default(),
        agent_id: None,
        event: AgentEvent::PermissionRequested {
            request: rid("r3"),
            tool: "Bash".into(),
            target: "Bash · cargo test && rm -rf build".into(),
            ask: Ask::default(),
        },
    });
    assert_eq!(
        visible(&mut s, other, now),
        vec![Effect::AckPermission(rid("r3"))]
    );
    assert!(!s.pending.is_empty());
}

#[test]
fn a_cut_target_gets_no_always() {
    let mut s = State::default();
    let now = Instant::now();
    let cut = |id: &str| {
        agent(
            "a",
            AgentEvent::PermissionRequested {
                request: rid(id),
                tool: "Bash".into(),
                target: "Bash · echo ok…".into(),
                ask: Ask {
                    full: Some("echo ok\ncurl x | sh".into()),
                    cut: true,
                    ..Ask::default()
                },
            },
        )
    };
    visible(&mut s, cut("r1"), now);
    assert!(visible(&mut s, always("r1"), now).is_empty());
    assert!(s.rules.is_empty());
    assert_eq!(s.pending.len(), 1);

    // Not even a rule saved before, word for word the same target.
    visible(
        &mut s,
        Input::SetRules(vec![Rule {
            agent: AgentKind::Claude,
            cwd: "/home/me/vults".into(),
            tool: "Bash".into(),
            target: "Bash · echo ok…".into(),
        }]),
        now,
    );
    assert_eq!(
        visible(&mut s, cut("r2"), now),
        vec![Effect::AckPermission(rid("r2"))]
    );
}

#[test]
fn a_rule_is_scoped_to_its_folder() {
    let mut s = State::default();
    let now = Instant::now();
    visible(
        &mut s,
        Input::SetRules(vec![Rule {
            agent: AgentKind::Claude,
            cwd: "/elsewhere".into(),
            tool: "Bash".into(),
            target: "Bash · cargo test".into(),
        }]),
        now,
    );
    // Same tool and target, but this session works in another folder.
    assert_eq!(
        visible(&mut s, requested("a", "r1"), now),
        vec![Effect::AckPermission(rid("r1"))]
    );
}

#[test]
fn always_on_a_gone_card_does_nothing() {
    let mut s = State::default();
    assert!(visible(&mut s, always("ghost"), Instant::now()).is_empty());
    assert!(s.rules.is_empty());
}

#[test]
fn a_note_explains_the_state_and_leaves_with_it() {
    let mut s = State::default();
    let now = Instant::now();
    let note = |s: &State| s.view().sessions[0].note.clone();
    visible(
        &mut s,
        agent(
            "a",
            AgentEvent::Question {
                message: " Which theme? ".into(),
            },
        ),
        now,
    );
    assert_eq!(note(&s).as_deref(), Some("Which theme?"));
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    assert_eq!(note(&s), None);
    visible(
        &mut s,
        agent(
            "a",
            AgentEvent::Stopped {
                message: Some("All tests pass.".into()),
            },
        ),
        now,
    );
    assert_eq!(note(&s).as_deref(), Some("All tests pass."));
    // A subagent finishing late does not change what the session said.
    visible(&mut s, agent("a", AgentEvent::SubagentStopped), now);
    assert_eq!(note(&s).as_deref(), Some("All tests pass."));
    visible(
        &mut s,
        agent(
            "a",
            AgentEvent::StopFailed {
                error: Some("   ".into()),
            },
        ),
        now,
    );
    assert_eq!(note(&s), None);
}

#[test]
fn the_editor_comes_from_the_terminal() {
    let term = |vars: &[(&str, &str)]| Terminal {
        env: vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        ..Default::default()
    };
    assert_eq!(view::editor(&term(&[("TERM_PROGRAM", "kitty")])), None);
    assert_eq!(
        view::editor(&term(&[("TERM_PROGRAM", "vscode")])),
        Some("VS Code")
    );
    assert_eq!(
        view::editor(&term(&[
            ("TERM_PROGRAM", "vscode"),
            ("VSCODE_GIT_ASKPASS_NODE", "/usr/share/code/code"),
        ])),
        Some("VS Code")
    );
    assert_eq!(
        view::editor(&term(&[
            ("TERM_PROGRAM", "vscode"),
            (
                "VSCODE_GIT_ASKPASS_NODE",
                "/tmp/.mount_CursorAbc/usr/share/cursor/Cursor"
            ),
        ])),
        Some("Cursor")
    );
    assert_eq!(
        view::editor(&term(&[("TERM_PROGRAM", "vscode"), ("CURSOR_TRACE_ID", "f00")])),
        Some("Cursor")
    );
    // A folder named after Cursor is not the editor: only the binary's own name counts.
    assert_eq!(
        view::editor(&term(&[
            ("TERM_PROGRAM", "vscode"),
            ("VSCODE_GIT_ASKPASS_NODE", "/home/me/cursor-tools/code"),
        ])),
        Some("VS Code")
    );
}

// ---- the flock: which vulture each session's bird is ----

fn in_project(session: &str, project: &str) -> Input {
    in_project_event(session, project, AgentEvent::SessionStarted)
}

fn in_project_event(session: &str, project: &str, event: AgentEvent) -> Input {
    let Input::Agent(mut u) = agent(session, event) else {
        unreachable!()
    };
    u.cwd = Some(format!("/home/me/{project}"));
    Input::Agent(u)
}

fn species_of(s: &State, id: &str) -> &'static str {
    s.view().sessions.iter().find(|v| v.id == id).unwrap().species
}

#[test]
fn a_session_keeps_its_species_while_it_lives() {
    let mut s = State {
        season: 7,
        ..State::default()
    };
    let now = Instant::now();
    visible(&mut s, in_project("a", "site"), now);
    let first = species_of(&s, "a");
    assert!(flock::POOL.contains(&first));
    visible(
        &mut s,
        agent("a", AgentEvent::Stopped { message: None }),
        now + Duration::from_secs(60),
    );
    assert_eq!(species_of(&s, "a"), first);
}

#[test]
fn a_new_season_draws_a_new_flock_from_the_pool() {
    let ids: Vec<String> = (0..40).map(|i| format!("session-{i}")).collect();
    let draw = |season| {
        ids.iter()
            .map(|id| flock::drawn(&flock::POOL, season, id))
            .collect::<Vec<_>>()
    };
    let (one, two) = (draw(1), draw(2));
    assert_ne!(one, two);
    for species in one.iter().chain(&two) {
        assert!(flock::POOL.contains(species), "{species} is not in the pool");
    }
    // Every species of the pool shows up in a flock this size.
    for species in flock::POOL {
        assert!(one.contains(&species), "{species} never drawn");
    }
    // A season reshuffles the flock, not just its labels: six sessions see many different flocks.
    let six = &ids[..6];
    let flocks: std::collections::BTreeSet<Vec<&str>> = (0..48u64)
        .map(|season| {
            six.iter()
                .map(|id| flock::drawn(&flock::POOL, season, id))
                .collect()
        })
        .collect();
    assert!(flocks.len() > 4, "only {} flocks across 48 seasons", flocks.len());
}

#[test]
fn the_oldest_session_of_a_busy_project_is_king_and_stays_king() {
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, in_project("a", "api"), t0);
    visible(&mut s, in_project("b", "api"), t0 + Duration::from_secs(1));
    assert!(
        s.view().sessions.iter().all(|v| v.species != flock::KING),
        "two sessions: no king yet"
    );
    visible(&mut s, in_project("c", "api"), t0 + Duration::from_secs(2));
    visible(&mut s, in_project("x", "site"), t0 + Duration::from_secs(3));
    assert_eq!(species_of(&s, "a"), flock::KING);
    for id in ["b", "c", "x"] {
        assert_ne!(species_of(&s, id), flock::KING, "{id}");
    }
    // Activity elsewhere in the project does not move the crown.
    visible(
        &mut s,
        in_project_event("c", "api", AgentEvent::Stopped { message: None }),
        t0 + Duration::from_secs(9),
    );
    assert_eq!(species_of(&s, "a"), flock::KING);
    // The king leaves: the next oldest inherits, once the project still has three.
    visible(&mut s, in_project("d", "api"), t0 + Duration::from_secs(10));
    visible(
        &mut s,
        in_project_event("a", "api", AgentEvent::SessionEnded),
        t0 + Duration::from_secs(11),
    );
    assert_eq!(species_of(&s, "b"), flock::KING);
    // Down to two sessions: no king, and b draws like everyone else.
    visible(
        &mut s,
        in_project_event("c", "api", AgentEvent::SessionEnded),
        t0 + Duration::from_secs(12),
    );
    assert_ne!(species_of(&s, "b"), flock::KING);
}

#[test]
fn a_project_is_one_breed_and_the_same_at_every_start() {
    let run = |season| {
        let mut s = State {
            season,
            ..State::default()
        };
        let t0 = Instant::now();
        visible(&mut s, in_project("a", "site"), t0);
        visible(&mut s, in_project("b", "site"), t0 + Duration::from_secs(1));
        assert_eq!(species_of(&s, "a"), species_of(&s, "b"), "one project, one breed");
        species_of(&s, "a")
    };
    let first = run(1);
    for season in 2..20 {
        assert_eq!(run(season), first, "season {season}");
    }
    assert_eq!(first, flock::breed(&flock::POOL, "/home/me/site"));
}

#[test]
fn projects_on_the_wire_differ_while_the_pool_allows() {
    let mut s = State::default();
    let t0 = Instant::now();
    let projects = ["p0", "p1", "p2", "p3", "p4"];
    for (i, p) in projects.iter().enumerate() {
        visible(
            &mut s,
            in_project(&format!("s{i}"), p),
            t0 + Duration::from_secs(i as u64),
        );
    }
    let first_four: std::collections::BTreeSet<&str> =
        (0..4).map(|i| species_of(&s, &format!("s{i}"))).collect();
    assert_eq!(first_four.len(), 4, "Brazil's four species, one per project");
    // The fifth repeats one: its own breed, the pool has nothing left.
    assert_eq!(species_of(&s, "s4"), flock::breed(&flock::POOL, "/home/me/p4"));
}

#[test]
fn a_project_keeps_its_breed_while_it_lives_whoever_leaves() {
    // Two folders that draw the same breed: the second takes another, and keeps it once the
    // first is gone.
    let folders: Vec<String> = (0..200).map(|i| format!("q{i}")).collect();
    let target = flock::breed(&flock::POOL, "/home/me/q0");
    let twin = folders[1..]
        .iter()
        .find(|f| flock::breed(&flock::POOL, &format!("/home/me/{f}")) == target)
        .expect("a folder with the same breed");
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, in_project("a", "q0"), t0);
    visible(&mut s, in_project("b", twin), t0 + Duration::from_secs(1));
    assert_eq!(species_of(&s, "a"), target);
    let moved = species_of(&s, "b");
    assert_ne!(moved, target, "no clash while the pool has room");
    visible(
        &mut s,
        in_project_event("a", "q0", AgentEvent::SessionEnded),
        t0 + Duration::from_secs(2),
    );
    assert_eq!(species_of(&s, "b"), moved, "a live flock never changes species");
    // A new session of the flock joins its breed.
    visible(&mut s, in_project("c", twin), t0 + Duration::from_secs(3));
    assert_eq!(species_of(&s, "c"), moved);
}

#[test]
fn a_session_with_no_folder_draws_its_own() {
    let mut s = State {
        season: 5,
        ..State::default()
    };
    let Input::Agent(mut u) = agent("lone", AgentEvent::SessionStarted) else {
        unreachable!()
    };
    u.cwd = None;
    visible(&mut s, Input::Agent(u), Instant::now());
    assert_eq!(species_of(&s, "lone"), flock::drawn(&flock::POOL, 5, "lone"));
    assert!(s.breeds.is_empty());
}

#[test]
fn a_chosen_bird_wins_over_the_draw_and_the_draws_keep_clear_of_it() {
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, in_project("a", "site"), t0);
    let drawn = species_of(&s, "a");
    assert!(!s.view().sessions[0].bird_chosen);
    // From every species, even outside the pool, at once, and saved.
    let effects = visible(
        &mut s,
        Input::User(Intent::SetProjectBird {
            session: key("a"),
            species: Some("gypaetus".into()),
        }),
        t0 + Duration::from_secs(1),
    );
    assert_eq!(species_of(&s, "a"), "gypaetus");
    assert!(s.view().sessions[0].bird_chosen);
    assert!(
        matches!(&effects[..], [Effect::SaveProjects(p)] if p["/home/me/site"].species.as_deref() == Some("gypaetus"))
    );
    // A new session of the project is the chosen species too.
    visible(&mut s, in_project("b", "site"), t0 + Duration::from_secs(2));
    assert_eq!(species_of(&s, "b"), "gypaetus");
    // Back to the draw: the folder's own breed again.
    visible(
        &mut s,
        Input::User(Intent::SetProjectBird {
            session: key("a"),
            species: None,
        }),
        t0 + Duration::from_secs(3),
    );
    assert_eq!(species_of(&s, "a"), drawn);
    assert!(
        !s.projects.contains_key("/home/me/site"),
        "no choice left: forgotten"
    );
}

#[test]
fn the_king_and_unknown_species_are_never_a_projects_bird() {
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, in_project("a", "site"), t0);
    let drawn = species_of(&s, "a");
    for bad in ["papa", "dodo", ""] {
        visible(
            &mut s,
            Input::User(Intent::SetProjectBird {
                session: key("a"),
                species: Some(bad.into()),
            }),
            t0,
        );
        assert_eq!(species_of(&s, "a"), drawn, "{bad}");
    }
    // One in the file that is not a species is ignored too.
    let mut prefs = BTreeMap::new();
    prefs.insert(
        "/home/me/site".to_string(),
        ProjectPrefs {
            species: Some("dodo".into()),
            ..ProjectPrefs::default()
        },
    );
    visible(&mut s, Input::SetProjects(prefs), t0);
    assert_eq!(species_of(&s, "a"), drawn);
}

#[test]
fn a_drawn_flock_keeps_clear_of_a_chosen_one() {
    let mut s = State::default();
    let t0 = Instant::now();
    // Every Brazil species but the one "p1" draws is chosen by other projects: p1 still gets its own,
    // and a project choosing p1's breed leaves the drawn ones to the rest.
    let target = flock::breed(&flock::POOL, "/home/me/p1");
    let mut prefs = BTreeMap::new();
    prefs.insert(
        "/home/me/p0".to_string(),
        ProjectPrefs {
            species: Some(target.into()),
            ..ProjectPrefs::default()
        },
    );
    visible(&mut s, Input::SetProjects(prefs), t0);
    visible(&mut s, in_project("a", "p0"), t0);
    visible(&mut s, in_project("b", "p1"), t0 + Duration::from_secs(1));
    assert_eq!(species_of(&s, "a"), target);
    assert_ne!(species_of(&s, "b"), target, "the chosen species is taken");
}

#[test]
fn a_session_with_no_project_is_never_king() {
    let mut s = State::default();
    let t0 = Instant::now();
    for (i, id) in ["n1", "n2", "n3", "n4"].into_iter().enumerate() {
        let Input::Agent(mut u) = agent(id, AgentEvent::SessionStarted) else {
            unreachable!()
        };
        u.cwd = None;
        visible(&mut s, Input::Agent(u), t0 + Duration::from_secs(i as u64));
    }
    assert!(s.view().sessions.iter().all(|v| v.species != flock::KING));
}

#[test]
fn every_species_the_core_names_exists_in_the_renderer() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/src/character/flock/species.ts"
    );
    let species = std::fs::read_to_string(path).expect("the renderer's species");
    let all = [flock::Flock::Brazil, flock::Flock::Americas, flock::Flock::World];
    for id in all.iter().flat_map(|f| f.pool()).chain([&flock::KING]) {
        assert!(
            species.contains(&format!("id: \"{id}\"")),
            "{id} is not a species of the renderer"
        );
    }
}

#[test]
fn the_chosen_pool_is_what_the_flock_draws_from() {
    let mut s = State {
        season: 3,
        ..State::default()
    };
    let now = Instant::now();
    for i in 0..60 {
        visible(&mut s, in_project(&format!("s{i}"), &format!("p{i}")), now);
    }
    let drawn = |s: &State| s.view().sessions.iter().map(|v| v.species).collect::<Vec<_>>();
    assert!(drawn(&s).iter().all(|id| flock::POOL.contains(id)));
    visible(&mut s, Input::SetFlock(flock::Flock::World), now);
    let world = drawn(&s);
    assert!(world.iter().all(|id| flock::Flock::World.pool().contains(id)));
    assert!(
        world.iter().any(|id| !flock::POOL.contains(id)),
        "the world pool reaches past Brazil"
    );
}

fn edit_step(file: &str) -> AgentEvent {
    AgentEvent::ToolStarted(Step {
        activity: Activity::Edit,
        tool: "Edit".into(),
        detail: Some(file.into()),
    })
}

fn edited(path: &str, failed: bool) -> AgentEvent {
    AgentEvent::ToolFinished {
        failed,
        target: None,
        diff: Some(Diff {
            files: vec![FileDiff {
                path: path.into(),
                added: 2,
                removed: 1,
                hunks: vec![Hunk {
                    old_start: Some(3),
                    new_start: Some(3),
                    lines: vec!["-a".into(), "+b".into(), "+c".into()],
                }],
            }],
            cut: false,
        }),
    }
}

#[test]
fn a_finished_edit_keeps_its_diff_on_its_step() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", edit_step("main.rs")), now);
    visible(&mut s, agent("a", edit_step("lib.rs")), now);
    // Calls may finish out of order: each diff finds its own file's step.
    visible(&mut s, agent("a", edited("/w/src/main.rs", false)), now);
    let view = s.view();
    let summary = view.sessions[0].diffs.clone();
    assert_eq!(summary.len(), 2);
    assert_eq!(
        summary[0],
        Some(DiffSummary {
            step: 1,
            added: 2,
            removed: 1,
            files: 1
        })
    );
    assert_eq!(summary[1], None);
    assert_eq!(
        s.diff(&key("a"), 1).map(|d| d.files[0].path.as_str()),
        Some("/w/src/main.rs")
    );
    assert!(s.diff(&key("a"), 2).is_none());
    // A failed edit changed nothing; a diff with no step of its file goes nowhere.
    visible(&mut s, agent("a", edited("/w/src/lib.rs", true)), now);
    visible(&mut s, agent("a", edited("/w/other.rs", false)), now);
    assert!(s.diff(&key("a"), 2).is_none());
    assert_eq!(s.sessions[&key("a")].diffs.len(), 1);
}

#[test]
fn a_diff_goes_with_its_step() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", edit_step("main.rs")), now);
    visible(&mut s, agent("a", edited("/w/main.rs", false)), now);
    for _ in 0..MAX_STEPS {
        visible(&mut s, agent("a", edit_step("x.rs")), now);
    }
    assert!(s.diff(&key("a"), 1).is_none());
    assert!(s.sessions[&key("a")].diffs.is_empty());
    assert!(s.view().sessions[0].diffs.iter().all(Option::is_none));
}

#[test]
fn zeca_wears_the_look_of_the_day_the_app_gives() {
    use looks::{Date, Outfit};
    let mut s = State::default();
    let now = Instant::now();
    // No date yet: Auto shows nothing rather than guess.
    assert_eq!(s.view().look, None);
    visible(&mut s, Input::Today(Date::new(2026, 10, 4)), now);
    assert_eq!(s.view().look, Some(Outfit::WitchHat));
    // The next day comes in on a tick: the look follows it.
    visible(&mut s, Input::Today(Date::new(2026, 11, 2)), now);
    assert_eq!(s.view().look, None);
    visible(&mut s, Input::SetOutfit(Outfit::Sunglasses), now);
    assert_eq!(s.view().look, Some(Outfit::Sunglasses));
    visible(&mut s, Input::Today(Date::new(2026, 12, 25)), now);
    visible(&mut s, Input::SetOutfit(Outfit::None), now);
    assert_eq!(s.view().look, None);
    visible(&mut s, Input::SetOutfit(Outfit::Auto), now);
    assert_eq!(s.view().look, Some(Outfit::SantaHat));
}

#[test]
fn every_look_the_core_names_is_drawn() {
    use looks::Outfit::*;
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/src/character/zeca/zeca.json"
    );
    let sprites: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("Zeca's sprites")).expect("JSON");
    for look in [WitchHat, SantaHat, PartyHat, BunnyEars, Sunglasses] {
        let id = serde_json::to_value(look).expect("an id");
        let id = id.as_str().expect("a string id");
        assert!(sprites["looks"].get(id).is_some(), "{id} is not drawn in zeca.py");
    }
}

#[test]
fn every_look_has_one_place_in_the_pickers_groups() {
    use looks::Outfit::{self, *};
    // Exhaustive: a new look stops the build here; add it to the match and to `all` below.
    let named = |o: Outfit| match o {
        Auto | None | WitchHat | SantaHat | PartyHat | BunnyEars | Sunglasses | WestCoast | FittedCap
        | MountainHat | Headband | Dreads | FrontKnot | Durag | Crown | BucketHat | ClockChain
        | Headphones | ShutterShades | ChromeChain | EyePatch => o,
    };
    let all = [
        Auto,
        None,
        WitchHat,
        SantaHat,
        PartyHat,
        BunnyEars,
        Sunglasses,
        WestCoast,
        FittedCap,
        MountainHat,
        Headband,
        Dreads,
        FrontKnot,
        Durag,
        Crown,
        BucketHat,
        ClockChain,
        Headphones,
        ShutterShades,
        ChromeChain,
        EyePatch,
    ];
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/src/character/looks.ts");
    let ts = std::fs::read_to_string(path).expect("looks.ts");
    let groups = &ts[ts.find("LOOK_GROUPS").expect("the groups")..];
    for look in all.map(named) {
        let id = serde_json::to_value(look).expect("an id");
        let entry = format!("value: \"{}\"", id.as_str().expect("a string id"));
        assert_eq!(groups.matches(&entry).count(), 1, "{entry} in LOOK_GROUPS");
    }
}

fn session_view(s: &State, id: &str) -> SessionView {
    s.view()
        .sessions
        .into_iter()
        .find(|v| v.id == id)
        .expect("session in the view")
}

#[test]
fn each_status_asks_its_own_attention_in_order() {
    let cases = [
        (Status::Idle, Attention::Quiet),
        (Status::Thinking, Attention::Quiet),
        (Status::Working, Attention::Quiet),
        (Status::RateLimited, Attention::Info),
        (Status::Finished, Attention::Done),
        (Status::Failed, Attention::Failed),
        (Status::Approval, Attention::NeedsYou),
        (Status::Question, Attention::NeedsYou),
    ];
    for (status, attention) in cases {
        assert_eq!(status.attention(), attention, "{status:?}");
    }
    assert!(
        Attention::Quiet < Attention::Info
            && Attention::Info < Attention::Done
            && Attention::Done < Attention::Failed
            && Attention::Failed < Attention::NeedsYou
    );
    assert_eq!(
        serde_json::to_value(Attention::NeedsYou).expect("json"),
        "needs-you"
    );
}

#[test]
fn the_view_carries_each_sessions_attention_and_the_most_of_them() {
    let mut s = State::default();
    let now = Instant::now();
    assert_eq!(s.view().attention, Attention::Quiet);
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    assert_eq!(s.view().attention, Attention::Quiet);
    visible(&mut s, agent("b", AgentEvent::RateLimited), now);
    assert_eq!(s.view().attention, Attention::Info);
    visible(&mut s, agent("c", AgentEvent::Stopped { message: None }), now);
    assert_eq!(s.view().attention, Attention::Done);
    visible(&mut s, agent("d", AgentEvent::StopFailed { error: None }), now);
    assert_eq!(s.view().attention, Attention::Failed);
    visible(&mut s, requested("a", "r1"), now);
    let view = s.view();
    assert_eq!(view.attention, Attention::NeedsYou);
    let by_id = |id: &str| {
        view.sessions
            .iter()
            .find(|v| v.id == id)
            .expect("session")
            .attention
    };
    assert_eq!(
        ["a", "b", "c", "d"].map(by_id),
        [
            Attention::NeedsYou,
            Attention::Info,
            Attention::Done,
            Attention::Failed
        ]
    );
    // Answered: it works again, and the most is what the others ask.
    visible(&mut s, decide("r1", Decision::Allow), now);
    assert_eq!(s.view().attention, Attention::Failed);
}

#[test]
fn only_the_session_of_the_card_in_line_has_the_card() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, requested("b", "r2"), now);
    // Both need the user; only the first in line is on the card.
    assert!(session_view(&s, "a").card);
    let b = session_view(&s, "b");
    assert_eq!((b.card, b.attention), (false, Attention::NeedsYou));
    visible(&mut s, decide("r1", Decision::Deny), now);
    assert!(!session_view(&s, "a").card);
    assert!(session_view(&s, "b").card);
}

#[test]
fn a_question_card_is_the_card_and_a_terminal_question_is_not() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, asked("a", "q1"), now);
    assert!(session_view(&s, "a").card);

    // Asked in the terminal: it needs the user, but there is no card to show.
    visible(
        &mut s,
        agent(
            "b",
            AgentEvent::Question {
                message: "Go on?".into(),
            },
        ),
        now,
    );
    let b = session_view(&s, "b");
    assert_eq!((b.card, b.attention), (false, Attention::NeedsYou));

    // A subagent's permission in line while the session asks in the terminal: no card for it.
    let mut s = State::default();
    visible(&mut s, from_subagent("c", "sub-1", requested_event("r1")), now);
    visible(
        &mut s,
        agent(
            "c",
            AgentEvent::Question {
                message: "Which one?".into(),
            },
        ),
        now,
    );
    assert_eq!(s.pending.len(), 1);
    assert!(!session_view(&s, "c").card);
}

#[test]
fn a_card_whose_session_moved_on_is_not_shown() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, from_subagent("a", "sub-1", requested_event("r1")), now);
    assert!(session_view(&s, "a").card);
    // The main agent goes on working: the subagent's card still waits, but the session no
    // longer says so.
    let step = AgentEvent::ToolStarted(Step {
        activity: Activity::Read,
        tool: "Read".into(),
        detail: None,
    });
    visible(&mut s, agent("a", step), now);
    assert_eq!(s.pending.len(), 1);
    let a = session_view(&s, "a");
    assert_eq!((a.card, a.attention), (false, Attention::Quiet));
}

/// How each card left the line, newest first, as the view carries it.
fn outcomes(s: &State) -> Vec<(String, Outcome)> {
    s.view()
        .ended
        .into_iter()
        .map(|e| (e.request, e.outcome))
        .collect()
}

#[test]
fn a_card_answered_here_says_how() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, requested("a", "r2"), now);
    visible(&mut s, asked("b", "q1"), now);
    visible(&mut s, asked("b", "q2"), now);
    visible(&mut s, decide("r1", Decision::Allow), now);
    visible(&mut s, decide("r2", Decision::Deny), now);
    visible(&mut s, answer("q1", vec![one("Red"), one("S")]), now);
    visible(&mut s, Input::User(Intent::Release { request: rid("q2") }), now);
    assert_eq!(
        outcomes(&s),
        [
            ("q2".to_string(), Outcome::Released),
            ("q1".to_string(), Outcome::Answered),
            ("r2".to_string(), Outcome::Denied),
            ("r1".to_string(), Outcome::Allowed),
        ]
    );
    let last = &s.view().ended[0];
    assert_eq!((last.agent, last.session.as_str()), (AgentKind::Claude, "b"));
    // A click on a card that is gone ends nothing again.
    visible(&mut s, decide("r1", Decision::Deny), now);
    assert_eq!(outcomes(&s).len(), 4);
}

#[test]
fn always_ends_its_card_here_and_the_same_one_waiting_by_the_rule() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, requested("a", "r2"), now);
    visible(&mut s, always("r1"), now);
    assert_eq!(
        outcomes(&s),
        [
            ("r2".to_string(), Outcome::Rule),
            ("r1".to_string(), Outcome::Allowed)
        ]
    );
}

#[test]
fn a_card_settled_in_the_terminal_says_so() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    visible(&mut s, requested("b", "r2"), now);
    visible(&mut s, agent("b", AgentEvent::SessionEnded), now);
    assert_eq!(
        outcomes(&s),
        [
            ("r2".to_string(), Outcome::Terminal),
            ("r1".to_string(), Outcome::Terminal)
        ]
    );
}

#[test]
fn a_card_nobody_answered_expires() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, Input::Tick, now + PENDING_TTL / 2);
    assert!(outcomes(&s).is_empty());
    visible(&mut s, Input::Tick, now + PENDING_TTL);
    assert_eq!(outcomes(&s), [("r1".to_string(), Outcome::Expired)]);
}

#[test]
fn only_the_latest_ended_cards_are_kept() {
    let mut s = State::default();
    let now = Instant::now();
    for i in 0..MAX_ENDED + 3 {
        let id = format!("r{i}");
        visible(&mut s, requested("a", &id), now);
        visible(&mut s, decide(&id, Decision::Allow), now);
    }
    let kept = outcomes(&s);
    assert_eq!(kept.len(), MAX_ENDED);
    assert_eq!(kept[0].0, format!("r{}", MAX_ENDED + 2));
}

fn focus(session: Option<&str>) -> Input {
    Input::User(Intent::Focus {
        session: session.map(key),
    })
}

fn front(s: &State) -> Option<String> {
    s.view().front.map(|f| f.id)
}

#[test]
fn front_is_the_first_and_work_does_not_move_it() {
    let mut s = State::default();
    let now = Instant::now();
    assert_eq!(front(&s), None);
    visible(&mut s, agent("a", AgentEvent::SessionStarted), now);
    visible(
        &mut s,
        agent("b", AgentEvent::SessionStarted),
        now + Duration::from_secs(1),
    );
    assert_eq!(front(&s).as_deref(), Some("a"), "all idle: the first to arrive");
    visible(
        &mut s,
        agent("b", AgentEvent::PromptSubmitted),
        now + Duration::from_secs(2),
    );
    assert_eq!(
        front(&s).as_deref(),
        Some("a"),
        "work does not move it: the birds would trade places"
    );
    // The order is arrival, not the latest news: a busy session does not jump the line.
    visible(
        &mut s,
        agent("a", AgentEvent::PromptSubmitted),
        now + Duration::from_secs(3),
    );
    let ids: Vec<_> = s.view().sessions.iter().map(|v| v.id.clone()).collect();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(front(&s).as_deref(), Some("a"));
}

#[test]
fn focus_puts_a_session_in_front_until_it_leaves() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    visible(
        &mut s,
        agent("b", AgentEvent::SessionStarted),
        now + Duration::from_secs(1),
    );
    assert_eq!(front(&s).as_deref(), Some("a"));
    visible(&mut s, focus(Some("b")), now);
    assert_eq!(front(&s).as_deref(), Some("b"), "an idle session the user chose");
    assert_eq!(s.view().focus.map(|f| f.id).as_deref(), Some("b"));
    // A session that is not there takes nothing.
    visible(&mut s, focus(Some("gone")), now);
    assert_eq!(s.focus, Some(key("b")));
    visible(&mut s, agent("b", AgentEvent::SessionEnded), now);
    assert_eq!(s.focus, None, "forgotten when its session leaves");
    assert_eq!(front(&s).as_deref(), Some("a"));
}

#[test]
fn focus_is_forgotten_when_its_session_times_out() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    visible(&mut s, focus(Some("a")), now);
    visible(&mut s, Input::Tick, now + SESSION_TTL);
    assert!(s.sessions.is_empty());
    assert_eq!((s.focus.clone(), front(&s)), (None, None));
}

#[test]
fn focus_can_be_cleared() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    visible(
        &mut s,
        agent("b", AgentEvent::SessionStarted),
        now + Duration::from_secs(1),
    );
    visible(&mut s, focus(Some("b")), now);
    visible(&mut s, focus(None), now);
    assert_eq!(s.focus, None);
    assert_eq!(front(&s).as_deref(), Some("a"));
}

#[test]
fn a_waiting_card_wins_over_focus() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    visible(
        &mut s,
        agent("b", AgentEvent::SessionStarted),
        now + Duration::from_secs(1),
    );
    visible(&mut s, focus(Some("a")), now);
    visible(&mut s, requested("b", "r1"), now);
    assert_eq!(front(&s).as_deref(), Some("b"), "the card's session");
    // Focus chosen while it waits is kept for after: the card still comes first.
    visible(&mut s, focus(Some("a")), now);
    assert_eq!(front(&s).as_deref(), Some("b"));
    visible(&mut s, decide("r1", Decision::Allow), now);
    assert_eq!(
        front(&s).as_deref(),
        Some("a"),
        "the user's choice once the card is gone"
    );
}

#[test]
fn a_card_its_session_moved_past_does_not_take_the_front() {
    // A subagent's card while the main agent works on: not drawn (`card` false), so not in front.
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    visible(
        &mut s,
        agent("b", AgentEvent::SessionStarted),
        now + Duration::from_secs(1),
    );
    visible(&mut s, focus(Some("a")), now);
    let Input::Agent(asked) = requested("b", "r1") else {
        unreachable!()
    };
    visible(&mut s, from_subagent("b", "s1", asked.event), now);
    visible(&mut s, agent("b", AgentEvent::PromptSubmitted), now);
    assert_eq!(s.pending.len(), 1);
    assert_eq!(front(&s).as_deref(), Some("a"));
}

#[test]
fn focus_answers_nothing() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    assert!(visible(&mut s, focus(Some("a")), now).is_empty());
    assert!(visible(&mut s, focus(None), now).is_empty());
    assert_eq!(s.pending.len(), 1);
}

#[test]
fn going_to_a_card_in_line_brings_it_first_and_answers_nothing() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, asked("b", "q1"), now + Duration::from_secs(1));
    assert_eq!(s.view().approval.map(|a| a.request), Some("r1".into()));
    let view = s.view();
    assert!(view.sessions.iter().all(|v| v.waiting), "both cards wait");
    assert!(visible(&mut s, focus(Some("b")), now).is_empty());
    assert_eq!(s.view().approval.map(|a| a.request), Some("q1".into()));
    assert_eq!(front(&s).as_deref(), Some("b"));
    // Only the order changed: each still keeps its own deadline.
    let effects = visible(&mut s, Input::Tick, now + PENDING_TTL);
    assert_eq!(released(&effects), vec![rid("r1")]);
    assert_eq!(s.pending.len(), 1);
}

fn diffed(s: &mut State, cwd: Option<&str>, path: &str) {
    let now = Instant::now();
    let mut with = |event| {
        let Input::Agent(mut u) = agent("a", event) else {
            unreachable!()
        };
        u.cwd = cwd.map(Into::into);
        visible(s, Input::Agent(u), now);
    };
    with(edit_step("main.rs"));
    with(edited(path, false));
}

fn open_file(step: u32, file: usize) -> Input {
    Input::User(Intent::OpenFile {
        session: key("a"),
        step,
        file,
    })
}

#[test]
fn a_quick_action_opens_the_folder_or_a_changed_file_by_absolute_path() {
    let mut s = State::default();
    diffed(&mut s, Some("/w"), "/w/src/main.rs");
    let folder = Input::User(Intent::OpenFolder { session: key("a") });
    assert_eq!(
        visible(&mut s, folder, Instant::now()),
        vec![Effect::OpenFolder("/w".into())]
    );
    assert_eq!(
        visible(&mut s, open_file(1, 0), Instant::now()),
        vec![Effect::OpenFile {
            path: "/w/src/main.rs".into(),
            line: Some(3)
        }]
    );
    // Past the hunk's leading context, at the first changed line.
    let mut s2 = State::default();
    let now = Instant::now();
    visible(&mut s2, agent("a", edit_step("main.rs")), now);
    let mut with_context = edited("/w/src/main.rs", false);
    if let AgentEvent::ToolFinished { diff: Some(d), .. } = &mut with_context {
        d.files[0].hunks[0]
            .lines
            .splice(0..0, [" x".to_string(), " y".to_string()]);
    }
    visible(&mut s2, agent("a", with_context), now);
    assert_eq!(
        visible(&mut s2, open_file(1, 0), now),
        vec![Effect::OpenFile {
            path: "/w/src/main.rs".into(),
            line: Some(5)
        }]
    );
    // No such step or file, or another session: nothing.
    for input in [
        open_file(2, 0),
        open_file(1, 1),
        Input::User(Intent::OpenFolder { session: key("gone") }),
    ] {
        assert!(visible(&mut s, input, Instant::now()).is_empty());
    }

    // A file named from the project's folder is found in it.
    let mut s = State::default();
    diffed(&mut s, Some("/w/"), "src/main.rs");
    assert_eq!(
        visible(&mut s, open_file(1, 0), Instant::now()),
        vec![Effect::OpenFile {
            path: "/w/src/main.rs".into(),
            line: Some(3)
        }]
    );
    // With no absolute folder to start from, a relative path opens nothing: it could be read as
    // an option (`-x`) or from the app's own folder.
    let mut s = State::default();
    diffed(&mut s, Some("w"), "-x/main.rs");
    assert!(visible(&mut s, open_file(1, 0), Instant::now()).is_empty());
    let folder = Input::User(Intent::OpenFolder { session: key("a") });
    assert!(visible(&mut s, folder, Instant::now()).is_empty());
}

#[test]
fn open_terminal_raises_a_window_on_kde_with_the_agents_process() {
    let mut s = State::default();
    visible(&mut s, agent("a", AgentEvent::SessionStarted), Instant::now());
    assert!(!session_view(&s, "a").raise, "no desktop said");
    let Input::Agent(mut u) = agent("a", AgentEvent::PromptSubmitted) else {
        unreachable!()
    };
    u.terminal.env.insert("XDG_CURRENT_DESKTOP".into(), "KDE".into());
    visible(&mut s, Input::Agent(u.clone()), Instant::now());
    assert!(session_view(&s, "a").raise);
    u.terminal.pid = None;
    u.terminal
        .env
        .insert("XDG_CURRENT_DESKTOP".into(), "GNOME".into());
    visible(&mut s, Input::Agent(u.clone()), Instant::now());
    assert!(!session_view(&s, "a").raise);
    // A multiplexer's pane is brought forward on any desktop.
    u.terminal.env.insert("TMUX_PANE".into(), "%3".into());
    visible(&mut s, Input::Agent(u), Instant::now());
    assert!(session_view(&s, "a").raise);
}

#[test]
fn open_terminal_raises_a_window_in_any_x11_session_but_promises_no_wayland_one() {
    let raise_with = |pid: Option<u32>, vars: &[(&str, &str)]| {
        let mut s = State::default();
        let Input::Agent(mut u) = agent("a", AgentEvent::PromptSubmitted) else {
            unreachable!()
        };
        u.terminal.pid = pid;
        u.terminal.env = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        visible(&mut s, Input::Agent(u), Instant::now());
        session_view(&s, "a").raise
    };
    let pid = Some(4242);
    let xfce = [
        ("XDG_CURRENT_DESKTOP", "XFCE"),
        ("XDG_SESSION_TYPE", "x11"),
        ("DISPLAY", ":0"),
    ];
    assert!(raise_with(pid, &xfce));
    assert!(
        raise_with(pid, &[("DISPLAY", ":0")]),
        "an X11 session that sets no type"
    );
    let plasma = [("XDG_CURRENT_DESKTOP", "KDE"), ("XDG_SESSION_TYPE", "wayland")];
    assert!(raise_with(pid, &plasma));
    // XWayland's DISPLAY is not enough on GNOME or wlroots: native windows can't be raised.
    let gnome = [
        ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
        ("XDG_SESSION_TYPE", "wayland"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("DISPLAY", ":0"),
    ];
    assert!(!raise_with(pid, &gnome));
    assert!(!raise_with(None, &xfce), "no process to look for");
    // kitty is reached only through its remote control socket.
    assert!(!raise_with(None, &[("KITTY_WINDOW_ID", "3")]));
    let kitty = [("KITTY_WINDOW_ID", "3"), ("KITTY_LISTEN_ON", "unix:/tmp/kitty")];
    assert!(raise_with(None, &kitty));
}

#[test]
fn going_to_a_card_that_would_not_be_drawn_leaves_the_shown_one() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, requested("b", "r2"), now + Duration::from_secs(1));
    // A subagent of b works on: its card stays in line, but b no longer waits on it.
    let step = AgentEvent::ToolStarted(Step {
        activity: Activity::Read,
        tool: "Read".into(),
        detail: None,
    });
    visible(&mut s, from_subagent("b", "s1", step), now);
    assert_eq!(s.pending.len(), 2);
    assert!(!session_view(&s, "b").waiting);
    visible(&mut s, focus(Some("b")), now);
    let shown = s.view();
    assert_eq!(shown.approval.map(|a| a.request), Some("r1".into()));
    assert!(
        shown.sessions.iter().any(|v| v.card),
        "a's card still has its host"
    );
}

fn three(s: &mut State, now: Instant) {
    for (i, id) in ["a", "b", "c"].into_iter().enumerate() {
        visible(
            s,
            agent(id, AgentEvent::SessionStarted),
            now + Duration::from_secs(i as u64),
        );
    }
}

#[test]
fn next_and_previous_walk_the_view_order_and_wrap() {
    let mut s = State::default();
    let now = Instant::now();
    let next = || Input::User(Intent::FocusNext);
    let previous = || Input::User(Intent::FocusPrevious);
    // Nobody there: nothing to move.
    assert!(visible(&mut s, next(), now).is_empty());
    assert_eq!(s.focus, None);
    three(&mut s, now);
    assert_eq!(front(&s).as_deref(), Some("a"));
    let mut walked = Vec::new();
    for _ in 0..4 {
        visible(&mut s, next(), now);
        walked.push(front(&s).unwrap());
    }
    assert_eq!(walked, ["b", "c", "a", "b"]);
    let mut walked = Vec::new();
    for _ in 0..3 {
        visible(&mut s, previous(), now);
        walked.push(front(&s).unwrap());
    }
    assert_eq!(walked, ["a", "c", "b"]);
    // The focused session leaves: the walk goes on from core's front again.
    visible(&mut s, agent("b", AgentEvent::SessionEnded), now);
    assert_eq!(s.focus, None);
    visible(&mut s, next(), now);
    assert_eq!(front(&s).as_deref(), Some("c"));
}

#[test]
fn next_starts_from_the_session_in_front() {
    let mut s = State::default();
    let now = Instant::now();
    three(&mut s, now);
    // Nothing chosen, "a" (the first) is in front, even with "b" at work: next goes on from it.
    visible(&mut s, agent("b", AgentEvent::PromptSubmitted), now);
    visible(&mut s, Input::User(Intent::FocusNext), now);
    assert_eq!(front(&s).as_deref(), Some("b"));
}

#[test]
fn next_walks_behind_a_waiting_card() {
    let mut s = State::default();
    let now = Instant::now();
    three(&mut s, now);
    visible(&mut s, requested("b", "r1"), now);
    visible(&mut s, Input::User(Intent::FocusNext), now);
    assert_eq!(front(&s).as_deref(), Some("b"), "the card stays in front");
    assert_eq!(s.focus, Some(key("c")));
    visible(&mut s, Input::User(Intent::FocusNext), now);
    assert_eq!(
        s.focus,
        Some(key("a")),
        "each press moves on, not back to the card"
    );
    assert_eq!(s.pending.len(), 1);
    visible(&mut s, decide("r1", Decision::Deny), now);
    assert_eq!(front(&s).as_deref(), Some("a"));
}

// ── Desktop notifications ────────────────────────────────────────────────────

use notify::{Change, Kind, Notifier, Prefs};

const ON: Prefs = Prefs { on: true };

/// An empty state in this preset.
fn in_preset(presence: Presence) -> State {
    State {
        presence,
        ..State::default()
    }
}

/// What each change does, as (session, kind) for a show and (session, None) for a withdrawal.
fn notes(changes: Vec<Change>) -> Vec<(String, Option<Kind>)> {
    changes
        .into_iter()
        .map(|c| match c {
            Change::Show { session, notice } => (session.session_id, Some(notice.kind)),
            Change::Withdraw { session } => (session.session_id, None),
        })
        .collect()
}

#[test]
fn a_card_notifies_at_once_by_the_panel() {
    let now = Instant::now();
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    visible(&mut s, requested("a", "r1"), now);
    let shown = n.update(&s, ON);
    let Some(Change::Show { notice, .. }) = shown.first() else {
        panic!("no notification");
    };
    assert_eq!(notice.title, "vults needs you");
    assert_eq!(notice.body, "Bash · cargo test");
    assert!(n.update(&s, ON).is_empty(), "one per event");
    // Answered: it goes.
    visible(&mut s, decide("r1", Decision::Allow), now);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), None)]);
}

#[test]
fn at_the_top_of_the_screen_nothing_goes_to_the_desktop() {
    let t = Instant::now();
    for presence in [Presence::Island, Presence::Quiet] {
        let mut s = in_preset(presence);
        let mut n = Notifier::default();
        visible(&mut s, requested("a", "r1"), t);
        visible(&mut s, agent("b", AgentEvent::Stopped { message: None }), t);
        visible(&mut s, agent("c", AgentEvent::StopFailed { error: None }), t);
        working(&mut s, "d", t);
        // A card waiting to its end, a bird quiet for long: the island says it, with its sounds.
        visible(&mut s, Input::Tick, t + Duration::from_secs(100));
        visible(&mut s, Input::Tick, t + 15 * MIN);
        assert!(n.update(&s, ON).is_empty(), "{presence:?}");
    }
    // Shown by the panel, then withdrawn when the island comes back to the top.
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    visible(&mut s, requested("a", "r1"), t);
    assert_eq!(n.update(&s, ON).len(), 1);
    visible(&mut s, Input::SetPresence(Presence::Island), t);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), None)]);
}

#[test]
fn one_notification_per_session_replaced_and_withdrawn() {
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    assert!(n.update(&s, ON).is_empty(), "work is not news");
    let stopped = AgentEvent::Stopped {
        message: Some("All   tests\npass.".into()),
    };
    visible(&mut s, agent("a", stopped), now);
    let shown = n.update(&s, ON);
    let [Change::Show { notice, .. }] = shown.as_slice() else {
        panic!("{shown:?}");
    };
    assert_eq!(
        (notice.kind, notice.title.as_str(), notice.body.as_str()),
        (Kind::Finished, "vults finished", "All tests pass.")
    );
    // Back at work: the old news goes.
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), now);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), None)]);
    // A card, then a failure: each replaces the session's one notification.
    visible(&mut s, requested("a", "r1"), now);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), Some(Kind::NeedsYou))]);
    visible(&mut s, decide("r1", Decision::Allow), now);
    let failed = AgentEvent::StopFailed {
        error: Some("overloaded".into()),
    };
    visible(&mut s, agent("a", failed), now);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), Some(Kind::Failed))]);
    // The session leaves: so does its notification.
    visible(&mut s, agent("a", AgentEvent::SessionEnded), now);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), None)]);
}

#[test]
fn turning_notifications_off_withdraws_them_and_shows_nothing() {
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, agent("b", AgentEvent::StopFailed { error: None }), now);
    assert_eq!(n.update(&s, ON).len(), 2);
    let off = Prefs { on: false };
    assert_eq!(
        notes(n.update(&s, off)),
        vec![("a".into(), None), ("b".into(), None)]
    );
    visible(&mut s, requested("c", "r2"), now);
    assert!(n.update(&s, off).is_empty());
}

#[test]
fn news_from_while_paused_is_not_raised_on_resume() {
    let mut s = in_preset(Presence::Paused);
    let mut n = Notifier::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::Stopped { message: None }), now);
    assert!(n.update(&s, ON).is_empty(), "paused: nothing");
    visible(&mut s, Input::SetPresence(Presence::Panel), now);
    assert!(
        n.update(&s, ON).is_empty(),
        "old news stays quiet after the pause"
    );
    // Something new after it does notify.
    visible(&mut s, agent("a", AgentEvent::StopFailed { error: None }), now);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), Some(Kind::Failed))]);
    // The same with notifications switched off and on again.
    let off = Prefs { on: false };
    visible(&mut s, agent("b", AgentEvent::Stopped { message: None }), now);
    n.update(&s, off);
    assert!(
        n.update(&s, ON)
            .iter()
            .all(|c| !matches!(c, Change::Show { session, .. } if session.session_id == "b"))
    );
}

#[test]
fn a_long_note_is_cut_and_a_session_without_a_folder_is_named_by_its_agent() {
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    let now = Instant::now();
    let stopped = AgentEvent::Stopped {
        message: Some("word ".repeat(100)),
    };
    let Input::Agent(mut update) = agent("a", stopped) else {
        unreachable!()
    };
    update.cwd = None;
    visible(&mut s, Input::Agent(update), now);
    let shown = n.update(&s, ON);
    let [Change::Show { notice, .. }] = shown.as_slice() else {
        panic!("{shown:?}");
    };
    assert_eq!(notice.title, "Claude Code finished");
    assert_eq!(notice.body.chars().count(), 160);
    assert!(notice.body.ends_with('…'));
}

#[test]
fn a_notification_can_only_bring_the_card_up() {
    // Its one action is a quiet intent (rule 2): it puts the session in front, nothing more.
    let open = notify::open(&key("a"));
    assert!(quiet_index(&open).is_some(), "{open:?} may answer a card");
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, asked("b", "q1"), now);
    for session in ["a", "b", "gone"] {
        let effects = visible(&mut s, Input::User(notify::open(&key(session))), now);
        assert!(effects.is_empty(), "{effects:?}");
    }
    assert_eq!(s.pending.len(), 2, "both cards still wait");
}

// ── Presence presets (ADR 0009) ──────────────────────────────────────────────

fn acked(effects: &[Effect]) -> Vec<RequestId> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::AckPermission(r) => Some(r.clone()),
            _ => None,
        })
        .collect()
}

fn released(effects: &[Effect]) -> Vec<RequestId> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::ReleasePermission(r) => Some(r.clone()),
            _ => None,
        })
        .collect()
}

/// Every acknowledged card not yet answered or released waits in the line, and the first in
/// line is the island's card, its session's `card` set: the island opens on it in every preset
/// that acknowledges.
fn every_acked_card_has_its_host(s: &State, effects: &[Effect]) {
    let gone: Vec<&RequestId> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::ReleasePermission(r) | Effect::RespondPermission { request: r, .. } => Some(r),
            _ => None,
        })
        .collect();
    let acked = acked(effects);
    let open: Vec<&RequestId> = acked.iter().filter(|r| !gone.contains(r)).collect();
    for r in &open {
        assert!(
            s.pending.iter().any(|p| &p.request == *r),
            "{r:?} waits for nobody"
        );
    }
    let view = s.view();
    match open.first() {
        Some(first) => {
            let card = view
                .approval
                .as_ref()
                .expect("an acknowledged card on the island");
            assert_eq!(&card.request, &first.0);
            assert!(view.sessions.iter().any(|v| v.card && v.id == card.session));
        }
        None => assert_eq!(view.approval, None),
    }
}

fn working(s: &mut State, id: &str, at: Instant) {
    let run = AgentEvent::ToolStarted(Step {
        activity: Activity::Run,
        tool: "Bash".into(),
        detail: Some("make".into()),
    });
    visible(s, agent(id, run), at);
}

fn silent_at(s: &mut State, id: &str, at: Instant) -> Option<silence::Silence> {
    visible(s, Input::Tick, at);
    session_view(s, id).silent
}

fn hush(id: &str, hush: silence::Hush) -> Input {
    Input::User(Intent::Hush {
        session: key(id),
        hush,
    })
}

const MIN: Duration = Duration::from_secs(60);

#[test]
fn a_working_bird_goes_quiet_at_5_minutes_and_loud_at_15() {
    use silence::Silence::{Loud, Quiet};
    let mut s = State::default();
    let t = Instant::now();
    working(&mut s, "a", t);
    visible(&mut s, agent("b", AgentEvent::PromptSubmitted), t);
    assert_eq!(silent_at(&mut s, "a", t + 4 * MIN), None);
    assert_eq!(silent_at(&mut s, "a", t + 5 * MIN), Some(Quiet));
    assert_eq!(silent_at(&mut s, "a", t + 15 * MIN), Some(Loud));
    // Loud, it is worth a glance on every surface (the widget, the tray), as a rate limit is.
    assert_eq!(session_view(&s, "a").attention, Attention::Info);
    assert_eq!(s.view().attention, Attention::Info);
    // Thinking (a long reply) is not a stuck tool.
    assert_eq!(session_view(&s, "b").silent, None);
    // Any news and the flag goes.
    working(&mut s, "a", t + 16 * MIN);
    assert_eq!(session_view(&s, "a").silent, None);
    assert_eq!(silent_at(&mut s, "a", t + 20 * MIN), None);
}

#[test]
fn snooze_keep_going_and_dismiss_only_change_the_flag() {
    use silence::Hush::{Dismiss, KeepGoing, Snooze};
    use silence::Silence::{Loud, Quiet};
    let mut s = State::default();
    let t = Instant::now();
    working(&mut s, "a", t);
    assert_eq!(silent_at(&mut s, "a", t + 15 * MIN), Some(Loud));
    // Snoozed: gone for 15 minutes, then back as it is.
    assert!(visible(&mut s, hush("a", Snooze), t + 15 * MIN).is_empty());
    assert_eq!(session_view(&s, "a").silent, None);
    assert_eq!(silent_at(&mut s, "a", t + 29 * MIN), None);
    assert_eq!(silent_at(&mut s, "a", t + 30 * MIN), Some(Loud));
    // Keep going: nothing for 30 minutes, then the ladder again. The bird stays on the wire
    // meanwhile, past the silent session's 30 minutes.
    assert!(visible(&mut s, hush("a", KeepGoing), t + 30 * MIN).is_empty());
    assert_eq!(silent_at(&mut s, "a", t + 59 * MIN), None);
    assert_eq!(silent_at(&mut s, "a", t + 60 * MIN), Some(Quiet));
    assert_eq!(silent_at(&mut s, "a", t + 70 * MIN), Some(Loud));
    // Dismissed: not again in this run, however long.
    assert!(visible(&mut s, hush("a", Dismiss), t + 70 * MIN).is_empty());
    assert_eq!(silent_at(&mut s, "a", t + 80 * MIN), None);
    working(&mut s, "a", t + 81 * MIN);
    assert_eq!(silent_at(&mut s, "a", t + 90 * MIN), None, "same run");
    // A new prompt is a new run: it is watched again.
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), t + 91 * MIN);
    working(&mut s, "a", t + 91 * MIN);
    assert_eq!(silent_at(&mut s, "a", t + 96 * MIN), Some(Quiet));
}

#[test]
fn a_loud_bird_notifies_once_unless_its_project_is_muted() {
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    let t = Instant::now();
    working(&mut s, "a", t);
    visible(&mut s, Input::Tick, t + 5 * MIN);
    assert!(n.update(&s, ON).is_empty(), "quiet is only shown");
    visible(&mut s, Input::Tick, t + 15 * MIN);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), Some(Kind::Silent))]);
    assert!(n.update(&s, ON).is_empty(), "once");
    // Snoozed, the notification goes with the flag.
    visible(&mut s, hush("a", silence::Hush::Snooze), t + 16 * MIN);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), None)]);
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    working(&mut s, "a", t);
    visible(&mut s, pref("a", ProjectPref::Mute, true), t);
    visible(&mut s, Input::Tick, t + 15 * MIN);
    assert!(n.update(&s, ON).is_empty());
}

#[test]
fn a_waiting_card_opens_the_island_then_sounds_again_slowly() {
    use notify::{REMIND_EVERY, REMIND_FROM, ladder, reminders};
    let secs = Duration::from_secs;
    assert_eq!(reminders(secs(44)), 0);
    assert_eq!(reminders(REMIND_FROM), 1);
    assert_eq!(reminders(REMIND_FROM + REMIND_EVERY), 2);
    // The app wakes the core at each step, all within the card's life.
    assert_eq!(ladder(), vec![secs(45), secs(75), secs(105)]);

    let mut s = State::default();
    let mut n = Notifier::default();
    let t = Instant::now();
    visible(&mut s, requested("a", "r1"), t);
    let step = |s: &mut State, at| {
        visible(s, Input::Tick, t + at);
        s.view().approval.map(|a| a.reminders)
    };
    assert_eq!(step(&mut s, secs(1)), Some(0), "the island opens with its sound");
    assert!(n.update(&s, ON).is_empty());
    assert_eq!(step(&mut s, secs(20)), Some(0));
    assert!(n.update(&s, ON).is_empty(), "the island has it: no notification");
    assert_eq!(step(&mut s, secs(45)), Some(1), "then a sound again");
    assert_eq!(step(&mut s, secs(75)), Some(2));
    assert_eq!(step(&mut s, secs(105)), Some(3));
}

#[test]
fn do_not_disturb_silences_notifications_for_a_while_and_cards_still_show() {
    let secs = Duration::from_secs;
    for presence in Presence::ALL {
        let mut s = in_preset(presence);
        let mut n = Notifier::default();
        let t = Instant::now();
        visible(&mut s, Input::SetDnd(Some(t + secs(60))), t);
        assert!(s.view().dnd);
        let mut effects = visible(&mut s, agent("b", AgentEvent::Stopped { message: None }), t);
        effects.extend(visible(&mut s, requested("a", "r1"), t));
        effects.extend(visible(&mut s, Input::Tick, t + secs(30)));
        // The card still has its host and its notification; the news at rest waits.
        every_acked_card_has_its_host(&s, &effects);
        let shown = notes(n.update(&s, ON));
        // Only by the panel does a card notify; at the top the island shows it.
        let card = if presence == Presence::Panel {
            vec![("a".to_string(), Some(Kind::NeedsYou))]
        } else {
            vec![]
        };
        assert_eq!(shown, card, "{presence:?}");
        // It ends by itself: what finished meanwhile is old news, the card still waiting is not.
        visible(&mut s, Input::Tick, t + secs(60));
        assert!(!s.view().dnd);
        assert!(n.update(&s, ON).is_empty(), "{presence:?}");
        assert_eq!(s.pending.len(), usize::from(presence != Presence::Paused));
    }
    // A time already past is no do not disturb at all.
    let mut s = State::default();
    let t = Instant::now();
    visible(&mut s, Input::SetDnd(Some(t)), t + secs(1));
    assert!(!s.view().dnd);
}

fn lock(locked: bool, missed: &[&str]) -> Input {
    Input::Locked {
        locked,
        missed: missed.iter().map(|id| key(id)).collect(),
    }
}

#[test]
fn back_from_a_locked_screen_a_digest_tells_what_happened_once() {
    let mut s = State::default();
    let t = Instant::now();
    for id in ["a", "b", "c"] {
        working(&mut s, id, t);
    }
    visible(&mut s, lock(true, &[]), t);
    assert!(s.view().locked);
    visible(&mut s, agent("a", AgentEvent::Stopped { message: None }), t + MIN);
    visible(&mut s, agent("b", AgentEvent::Stopped { message: None }), t + MIN);
    visible(
        &mut s,
        agent("c", AgentEvent::StopFailed { error: None }),
        t + MIN,
    );
    visible(&mut s, requested("d", "r1"), t + 2 * MIN);
    visible(&mut s, lock(false, &[]), t + 14 * MIN);
    let view = s.view();
    assert!(!view.locked);
    let digest = view.digest.expect("a digest");
    assert_eq!(
        digest.text,
        "While you were away: 2 finished, 1 failed, 1 waits for you for 12 min."
    );
    assert_eq!((digest.finished, digest.failed, digest.waiting), (2, 1, 1));
    // Dismissed, it goes; it never answers the card.
    assert!(visible(&mut s, Input::User(Intent::DismissDigest), t + 15 * MIN).is_empty());
    assert!(s.view().digest.is_none());
    assert_eq!(s.pending.len(), 1);
    // A card already waiting before the lock is not news on return; nothing else happened.
    visible(&mut s, lock(true, &[]), t + 15 * MIN);
    visible(&mut s, lock(false, &[]), t + 16 * MIN);
    assert!(s.view().digest.is_none());
    // Nothing happened (the card answered): no digest. A new one has a new seq.
    visible(&mut s, decide("r1", Decision::Deny), t + 15 * MIN);
    visible(&mut s, lock(true, &[]), t + 16 * MIN);
    visible(&mut s, lock(false, &[]), t + 17 * MIN);
    assert!(s.view().digest.is_none());
    visible(&mut s, lock(true, &[]), t + 18 * MIN);
    visible(
        &mut s,
        agent("a", AgentEvent::StopFailed { error: None }),
        t + 19 * MIN,
    );
    visible(&mut s, lock(false, &[]), t + 20 * MIN);
    let again = s.view().digest.expect("a digest");
    assert!(again.seq > digest.seq);
    assert_eq!(
        (again.finished, again.failed),
        (0, 1),
        "a later end replaces the earlier"
    );
}

#[test]
fn news_the_notifications_held_back_and_a_pause_join_the_digest() {
    let mut s = State::default();
    let t = Instant::now();
    working(&mut s, "a", t);
    working(&mut s, "b", t);
    visible(&mut s, agent("a", AgentEvent::Stopped { message: None }), t);
    // Held back before the lock (notifications off, the user there): not news on return.
    visible(&mut s, lock(true, &[]), t + MIN);
    visible(&mut s, lock(false, &["a", "gone"]), t + 2 * MIN);
    assert!(s.view().digest.is_none(), "old news is not told again");
    // A muted project's end is not told either.
    visible(&mut s, in_project("m", "muted"), t);
    visible(&mut s, pref("m", ProjectPref::Mute, true), t);
    visible(&mut s, lock(true, &[]), t + 2 * MIN);
    visible(
        &mut s,
        in_project_event("m", "muted", AgentEvent::Stopped { message: None }),
        t + 3 * MIN,
    );
    visible(&mut s, lock(false, &["m"]), t + 4 * MIN);
    assert!(s.view().digest.is_none());
    // A pause is away too.
    visible(&mut s, Input::SetPresence(Presence::Paused), t + 5 * MIN);
    visible(
        &mut s,
        agent("b", AgentEvent::StopFailed { error: None }),
        t + 6 * MIN,
    );
    // Unlocked while still paused: the digest waits for the pause to end.
    visible(&mut s, lock(true, &[]), t + 7 * MIN);
    visible(&mut s, lock(false, &[]), t + 8 * MIN);
    assert!(s.view().digest.is_none());
    visible(&mut s, Input::SetPresence(Presence::Island), t + 9 * MIN);
    assert_eq!(
        s.view().digest.map(|d| d.text).as_deref(),
        Some("While you were away: 1 failed.")
    );
}

#[test]
fn the_digest_is_one_whole_sentence_per_case() {
    use away::Digest;
    let d = |finished, failed, waiting| Digest {
        seq: 1,
        finished,
        failed,
        waiting,
        waited: Duration::from_secs(5 * 60),
    };
    let say = |d: Digest| i18n::digest(i18n::Lang::En, &d);
    assert_eq!(say(d(1, 0, 0)), "While you were away: 1 finished.");
    assert_eq!(say(d(0, 2, 0)), "While you were away: 2 failed.");
    assert_eq!(say(d(0, 0, 1)), "While you were away: 1 waits for you for 5 min.");
    assert_eq!(
        say(d(3, 1, 2)),
        "While you were away: 3 finished, 1 failed, 2 wait for you, the first for 5 min."
    );
}

fn pref(session: &str, pref: ProjectPref, on: bool) -> Input {
    Input::User(Intent::SetProjectPref {
        session: key(session),
        pref,
        on,
    })
}

#[test]
fn a_hidden_or_muted_project_still_shows_its_card_in_every_preset() {
    for presence in Presence::ALL {
        let mut s = in_preset(presence);
        let now = Instant::now();
        visible(&mut s, agent("a", AgentEvent::SessionStarted), now);
        visible(&mut s, pref("a", ProjectPref::Hide, true), now);
        visible(&mut s, pref("a", ProjectPref::Mute, true), now);
        assert!(s.view().sessions.is_empty(), "hidden at rest");
        let effects = visible(&mut s, requested("a", "r1"), now);
        every_acked_card_has_its_host(&s, &effects);
        if presence != Presence::Paused {
            assert!(session_view(&s, "a").muted);
        }
        // Answered, it hides again.
        visible(&mut s, decide("r1", Decision::Allow), now);
        assert!(s.view().sessions.is_empty());
    }
}

#[test]
fn project_prefs_are_kept_by_folder_and_saved() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::SessionStarted), now);
    visible(&mut s, in_project("b", "site"), now + Duration::from_secs(1));
    let effects = visible(&mut s, pref("b", ProjectPref::Pin, true), now);
    let pinned = ProjectPrefs {
        pin: true,
        ..Default::default()
    };
    assert_eq!(
        effects,
        vec![Effect::SaveProjects(BTreeMap::from([(
            "/home/me/site".to_string(),
            pinned
        )]))]
    );
    // Pinned first, though it came later.
    let order: Vec<String> = s.view().sessions.iter().map(|v| v.id.clone()).collect();
    assert_eq!(order, vec!["b", "a"]);
    assert!(session_view(&s, "b").pinned);
    // A new session of the same folder is pinned too: the choice is the project's.
    visible(&mut s, in_project("c", "site"), now);
    assert!(session_view(&s, "c").pinned);
    // Every choice off: the project is forgotten.
    let effects = visible(&mut s, pref("b", ProjectPref::Pin, false), now);
    assert_eq!(effects, vec![Effect::SaveProjects(BTreeMap::new())]);
    // Hiding the session in front gives the front back.
    visible(&mut s, focus(Some("a")), now);
    visible(&mut s, pref("a", ProjectPref::Hide, true), now);
    assert_eq!(s.focus, None);
    assert!(s.view().sessions.iter().all(|v| v.id != "a"));
    // The settings bring a project back, through core, which saves it.
    let effects = visible(
        &mut s,
        Input::SetProject {
            cwd: "/home/me/vults".into(),
            prefs: ProjectPrefs::default(),
        },
        now,
    );
    assert_eq!(effects, vec![Effect::SaveProjects(BTreeMap::new())]);
    assert!(s.view().sessions.iter().any(|v| v.id == "a"));
}

#[test]
fn a_muted_or_hidden_project_notifies_only_its_card() {
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    let now = Instant::now();
    visible(&mut s, agent("a", AgentEvent::Stopped { message: None }), now);
    visible(&mut s, pref("a", ProjectPref::Mute, true), now);
    assert!(n.update(&s, ON).is_empty());
    visible(&mut s, requested("a", "r1"), now);
    assert_eq!(
        notes(n.update(&s, ON)),
        vec![("a".into(), Some(Kind::NeedsYou))],
        "its card is still news (ADR 0009)"
    );
    let mut s = in_preset(Presence::Panel);
    let mut n = Notifier::default();
    visible(&mut s, agent("a", AgentEvent::Stopped { message: None }), now);
    visible(&mut s, pref("a", ProjectPref::Hide, true), now);
    assert!(n.update(&s, ON).is_empty());
    visible(&mut s, requested("a", "r1"), now);
    assert_eq!(notes(n.update(&s, ON)), vec![("a".into(), Some(Kind::NeedsYou))]);
}

#[test]
fn in_every_preset_an_acknowledged_card_has_its_host_and_paused_never_acknowledges() {
    let rule = Rule {
        agent: AgentKind::Claude,
        cwd: "/home/me/vults".into(),
        tool: "Bash".into(),
        target: "Bash · cargo test".into(),
    };
    for from in Presence::ALL {
        for to in Presence::ALL {
            let mut s = in_preset(from);
            let now = Instant::now();
            let mut effects = visible(&mut s, requested("a", "r1"), now);
            effects.extend(visible(&mut s, asked("b", "q1"), now));
            if from == Presence::Paused {
                assert!(acked(&effects).is_empty(), "paused acknowledged {effects:?}");
                assert_eq!(
                    released(&effects),
                    vec![rid("r1"), rid("q1")],
                    "the terminal asks at once"
                );
                // The agent waits on its terminal: the session says so, with no card.
                assert_eq!(session_view(&s, "a").status, Status::Approval);
            } else {
                assert_eq!(acked(&effects), vec![rid("r1"), rid("q1")], "{from:?}");
            }
            every_acked_card_has_its_host(&s, &effects);
            // Switching keeps every acknowledged card on the island, or (paused) sends it on.
            effects.extend(visible(&mut s, Input::SetPresence(to), now));
            if to == Presence::Paused {
                assert!(s.pending.is_empty(), "{from:?} to paused kept a card waiting");
            }
            every_acked_card_has_its_host(&s, &effects);
            // A new request, one a rule answers included, is acknowledged only when not paused.
            visible(&mut s, Input::SetRules(vec![rule.clone()]), now);
            for request in [requested("c", "r2"), requested("a", "r3")] {
                let more = visible(&mut s, request, now);
                assert_eq!(
                    acked(&more).is_empty(),
                    to == Presence::Paused,
                    "{to:?}: {more:?}"
                );
                effects.extend(more);
            }
            every_acked_card_has_its_host(&s, &effects);
        }
    }
}

#[test]
fn pausing_sends_the_waiting_cards_to_the_terminal_as_released() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, asked("b", "q1"), now);
    let effects = visible(&mut s, Input::SetPresence(Presence::Paused), now);
    assert_eq!(released(&effects), vec![rid("r1"), rid("q1")]);
    assert_eq!(
        outcomes(&s),
        vec![("q1".into(), Outcome::Released), ("r1".into(), Outcome::Released)]
    );
    // Each agent asks in its terminal now, as one asking while paused does.
    assert_eq!(session_view(&s, "a").status, Status::Approval);
    assert_eq!(session_view(&s, "b").status, Status::Question);
    // Back from the pause: the next card is the island's again.
    visible(&mut s, Input::SetPresence(Presence::Quiet), now);
    assert_eq!(
        acked(&visible(&mut s, requested("a", "r2"), now)),
        vec![rid("r2")]
    );
}

#[test]
fn paused_shows_no_notification_and_quiet_waits_like_the_island() {
    let now = Instant::now();
    let mut s = in_preset(Presence::Paused);
    let mut n = Notifier::default();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, agent("b", AgentEvent::Stopped { message: None }), now);
    assert!(n.update(&s, ON).is_empty());
    // Unpaused, what finished meanwhile is old news: no late notification (the away digest, C5,
    // is where it belongs).
    visible(&mut s, Input::SetPresence(Presence::Quiet), now);
    assert!(n.update(&s, ON).is_empty());
}

// ── Turns (docs/guide/activity.md) ─────────────────────────────────────

fn turns_of(effects: &[Effect]) -> Vec<&turns::Turn> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Turn(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn run_step(command: &str) -> AgentEvent {
    AgentEvent::ToolStarted(Step {
        activity: Activity::Run,
        tool: "Bash".into(),
        detail: Some(command.into()),
    })
}

#[test]
fn a_turn_counts_what_its_prompt_set_going() {
    let mut s = State::default();
    let t0 = Instant::now();
    let at = |secs| t0 + Duration::from_secs(secs);
    visible(&mut s, agent("a", AgentEvent::SessionStarted), t0);
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), at(10));
    visible(&mut s, agent("a", run_step("cargo test")), at(20));
    visible(&mut s, agent("a", edit_step("main.rs")), at(30));
    visible(
        &mut s,
        agent("a", edited("/home/me/vults/main.rs", false)),
        at(31),
    );
    visible(&mut s, agent("a", edit_step("lib.rs")), at(40));
    visible(&mut s, agent("a", edited("/home/me/vults/lib.rs", false)), at(41));
    // The same file again: still two files.
    visible(&mut s, agent("a", edit_step("main.rs")), at(50));
    visible(
        &mut s,
        agent("a", edited("/home/me/vults/main.rs", false)),
        at(51),
    );
    // A failed edit changed nothing.
    visible(&mut s, agent("a", edit_step("x.rs")), at(55));
    visible(&mut s, agent("a", edited("/home/me/vults/x.rs", true)), at(56));
    visible(&mut s, requested("a", "r1"), at(60));
    visible(&mut s, decide("r1", Decision::Allow), at(61));
    visible(&mut s, requested("a", "r2"), at(62));
    visible(&mut s, decide("r2", Decision::Deny), at(63));
    visible(&mut s, asked("a", "q1"), at(70));
    visible(
        &mut s,
        answer("q1", vec![one("Red"), Answer::Many(vec!["S".into()])]),
        at(71),
    );
    let effects = visible(&mut s, agent("a", AgentEvent::Stopped { message: None }), at(100));
    assert_eq!(
        turns_of(&effects),
        [&turns::Turn {
            agent: AgentKind::Claude,
            project: "vults".into(),
            secs: 90,
            ago: Duration::ZERO,
            steps: 5,
            commands: 1,
            files: 2,
            added: 6,
            removed: 3,
            allowed: 1,
            denied: 1,
            answered: 1,
            questions: 1,
            failed: false,
        }]
    );
    // Stopping again, with no new prompt, is no turn.
    let effects = visible(&mut s, agent("a", AgentEvent::Stopped { message: None }), at(110));
    assert!(turns_of(&effects).is_empty());
}

#[test]
fn a_failed_turn_and_two_turns_in_one_session() {
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), t0);
    let effects = visible(
        &mut s,
        agent(
            "a",
            AgentEvent::StopFailed {
                error: Some("overloaded".into()),
            },
        ),
        t0 + Duration::from_secs(5),
    );
    let first = turns_of(&effects);
    assert!(first.len() == 1 && first[0].failed && first[0].secs == 5);
    visible(
        &mut s,
        agent("a", AgentEvent::PromptSubmitted),
        t0 + Duration::from_secs(60),
    );
    visible(&mut s, agent("a", run_step("ls")), t0 + Duration::from_secs(61));
    let effects = visible(
        &mut s,
        agent("a", AgentEvent::SessionEnded),
        t0 + Duration::from_secs(70),
    );
    let second = turns_of(&effects);
    assert!(second.len() == 1 && !second[0].failed && second[0].secs == 10 && second[0].commands == 1);
}

#[test]
fn a_session_that_dies_ends_its_turn_at_its_last_event() {
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), t0);
    visible(&mut s, agent("a", run_step("make")), t0 + Duration::from_secs(30));
    let later = t0 + Duration::from_secs(30) + SESSION_TTL;
    let effects = visible(&mut s, Input::Tick, later);
    let t = turns_of(&effects);
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].secs, t[0].ago), (30, SESSION_TTL));
    assert!(s.sessions.is_empty());
}

#[test]
fn a_turn_waiting_on_a_card_for_hours_ends_once_silent() {
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, agent("a", AgentEvent::PromptSubmitted), t0);
    visible(&mut s, requested("a", "r1"), t0);
    // The card keeps the session; its own TTL releases the card, then the session goes quiet.
    let mut effects = Vec::new();
    for minutes in (10..=150).step_by(10) {
        effects.extend(visible(
            &mut s,
            Input::Tick,
            t0 + Duration::from_secs(minutes * 60),
        ));
    }
    assert_eq!(turns_of(&effects).len(), 1, "one turn, ended once");
}

#[test]
fn hidden_and_muted_projects_still_count_and_no_path_is_kept() {
    let mut s = State::default();
    let t0 = Instant::now();
    visible(&mut s, in_project("a", "secret"), t0);
    visible(
        &mut s,
        Input::SetProject {
            cwd: "/home/me/secret".into(),
            prefs: ProjectPrefs {
                mute: true,
                hide: true,
                ..ProjectPrefs::default()
            },
        },
        t0,
    );
    visible(
        &mut s,
        in_project_event("a", "secret", AgentEvent::PromptSubmitted),
        t0,
    );
    visible(
        &mut s,
        in_project_event("a", "secret", run_step("rm -rf /home/me/secret/build")),
        t0,
    );
    let effects = visible(
        &mut s,
        in_project_event(
            "a",
            "secret",
            AgentEvent::Stopped {
                message: Some("Done.".into()),
            },
        ),
        t0 + Duration::from_secs(1),
    );
    let t = turns_of(&effects);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].project, "secret");
    let shown = format!("{:?}", t[0]);
    for leak in ["/home", "rm -rf", "Done."] {
        assert!(!shown.contains(leak), "{leak} in {shown}");
    }
}

// ── The Monday card (docs/guide/activity.md) ───────────────────────────

fn recap_due(monday: &str) -> Input {
    Input::Recap {
        monday: monday.into(),
        headline: "41 turns, 6 h 20 min with your agents, most on site.".into(),
    }
}

#[test]
fn the_monday_card_is_told_once_and_saved_when_read() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, recap_due("2026-10-05"), now);
    let v = s.view().recap.expect("the card");
    assert_eq!(
        v.text,
        "Last week: 41 turns, 6 h 20 min with your agents, most on site."
    );
    // The same week again (another tick) keeps the card as it is: the island opens once.
    visible(&mut s, recap_due("2026-10-05"), now);
    assert_eq!(s.view().recap.map(|c| c.seq), Some(v.seq));
    let effects = visible(&mut s, Input::User(Intent::DismissRecap), now);
    assert_eq!(effects, [Effect::RecapSeen("2026-10-05".into())]);
    assert!(s.view().recap.is_none());
    // Nothing left to dismiss: nothing saved.
    assert!(visible(&mut s, Input::User(Intent::DismissRecap), now).is_empty());
}

#[test]
fn a_waiting_card_comes_before_the_recap_and_paused_shows_none() {
    let mut s = State::default();
    let now = Instant::now();
    visible(&mut s, recap_due("2026-10-05"), now);
    visible(&mut s, requested("a", "r1"), now);
    assert!(s.view().recap.is_none(), "the permission first");
    visible(&mut s, decide("r1", Decision::Allow), now);
    assert!(s.view().recap.is_some(), "back once it is answered");
    visible(&mut s, Input::SetPresence(Presence::Paused), now);
    assert!(s.view().recap.is_none());
    visible(&mut s, Input::SetPresence(Presence::Quiet), now);
    assert!(
        s.view().recap.is_some(),
        "Quiet keeps it for when the island is opened"
    );
}

fn audits(effects: &[Effect]) -> Vec<(audit::Actor, audit::Act, String)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Audit(a) => Some((a.actor, a.act, a.target.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn every_answer_is_audited_with_who_and_on_what() {
    use audit::{Act, Actor};
    let now = Instant::now();
    let target = "Bash · cargo test".to_string();
    let cases: Vec<(Input, (Actor, Act))> = vec![
        (decide("r1", Decision::Allow), (Actor::Human, Act::Allow)),
        (decide("r1", Decision::Deny), (Actor::Human, Act::Deny)),
        (
            Input::User(Intent::Release { request: rid("r1") }),
            (Actor::Human, Act::Release),
        ),
    ];
    for (input, want) in cases {
        let mut s = State::default();
        reduce(&mut s, requested("a", "r1"), now);
        assert_eq!(
            audits(&reduce(&mut s, input, now)),
            vec![(want.0, want.1, target.clone())]
        );
    }

    // Always: the user's click, then the rule answering the same request waiting again.
    let mut s = State::default();
    reduce(&mut s, requested("a", "r1"), now);
    reduce(&mut s, from_subagent("a", "sub-1", requested_event("r2")), now);
    let effects = reduce(
        &mut s,
        Input::User(Intent::DecideAlways { request: rid("r1") }),
        now,
    );
    assert_eq!(
        audits(&effects),
        vec![
            (Actor::Human, Act::AlwaysAllow, target.clone()),
            (Actor::Rule, Act::Allow, target.clone()),
        ]
    );
    // The saved rule answers the next identical request at once, with no card.
    let effects = reduce(&mut s, requested("a", "r3"), now);
    assert_eq!(audits(&effects), vec![(Actor::Rule, Act::Allow, target.clone())]);

    // A question card answered: kept, without the answers.
    let mut s = State::default();
    reduce(&mut s, asked("a", "q1"), now);
    let effects = reduce(&mut s, answer("q1", vec![one("Red"), one("S")]), now);
    assert_eq!(
        audits(&effects),
        vec![(Actor::Human, Act::Answer, "AskUserQuestion".to_string())]
    );

    // Nobody answered in time.
    let mut s = State::default();
    reduce(&mut s, requested("a", "r1"), now);
    let effects = reduce(&mut s, Input::Tick, now + PENDING_TTL);
    assert_eq!(audits(&effects), vec![(Actor::System, Act::Expire, target)]);
}

#[test]
fn an_answer_is_never_without_its_audit_line() {
    // Every RespondPermission and AnswerQuestion `reduce` gives, whatever the input, comes with
    // one audit line; a click on a gone card gives neither.
    let now = Instant::now();
    let mut s = State::default();
    let inputs = vec![
        requested("a", "r1"),
        requested("b", "r2"),
        asked("c", "q1"),
        decide("gone", Decision::Allow),
        decide("r1", Decision::Allow),
        decide("r1", Decision::Allow),
        Input::User(Intent::DecideAlways { request: rid("r2") }),
        requested("b", "r4"),
        answer("q1", vec![one("Blue"), one("M")]),
        requested("a", "r5"),
        Input::Tick,
    ];
    for input in inputs {
        let effects = reduce(&mut s, input, now);
        let answers = effects
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Effect::RespondPermission { .. } | Effect::AnswerQuestion { .. }
                )
            })
            .count();
        let lines = audits(&effects)
            .iter()
            .filter(|(_, act, _)| {
                matches!(
                    act,
                    audit::Act::Allow | audit::Act::Deny | audit::Act::AlwaysAllow | audit::Act::Answer
                )
            })
            .count();
        assert_eq!(answers, lines, "{effects:?}");
    }
}

#[test]
fn pausing_releases_waiting_cards_on_record() {
    let now = Instant::now();
    let mut s = State::default();
    reduce(&mut s, requested("a", "r1"), now);
    let effects = reduce(&mut s, Input::SetPresence(Presence::Paused), now);
    assert_eq!(
        audits(&effects),
        vec![(
            audit::Actor::System,
            audit::Act::Release,
            "Bash · cargo test".to_string()
        )]
    );
}

#[test]
fn every_waiting_card_is_an_open_offer_until_it_leaves() {
    let now = Instant::now();
    let mut s = State::default();
    visible(&mut s, requested("a", "r1"), now);
    visible(&mut s, asked("b", "q1"), now);
    assert!(s.ledger.is_open(&rid("r1")) && s.ledger.is_open(&rid("q1")));
    visible(&mut s, decide("r1", Decision::Allow), now);
    assert!(!s.ledger.is_open(&rid("r1")));
    // Sent to the terminal: nothing here may answer it any more.
    visible(&mut s, Input::User(Intent::Release { request: rid("q1") }), now);
    assert!(!s.ledger.is_open(&rid("q1")));
}

#[test]
fn a_click_after_the_hook_stopped_waiting_answers_nothing() {
    let now = Instant::now();
    let mut s = State::default();
    visible(&mut s, requested("a", "r1"), now);
    // The tick that releases it has not come yet, but the hook is gone.
    let late = now + PENDING_TTL;
    assert!(visible(&mut s, decide("r1", Decision::Allow), late).is_empty());
    assert_eq!(
        visible(&mut s, Input::Tick, late),
        vec![Effect::ReleasePermission(rid("r1"))]
    );
    // Just in time still counts.
    visible(&mut s, requested("a", "r2"), now);
    let in_time = now + PENDING_TTL - Duration::from_millis(1);
    assert_eq!(
        visible(&mut s, decide("r2", Decision::Allow), in_time),
        vec![Effect::RespondPermission {
            request: rid("r2"),
            decision: Decision::Allow
        }]
    );
}

#[test]
fn a_misaimed_click_never_spends_the_cards_offer() {
    let now = Instant::now();
    let mut s = State::default();
    visible(&mut s, asked("a", "q1"), now);
    // Allow is not an answer to a question: it must leave the question answerable.
    assert!(visible(&mut s, decide("q1", Decision::Allow), now).is_empty());
    assert!(visible(&mut s, answer("q1", vec![]), now).is_empty());
    assert!(s.ledger.is_open(&rid("q1")));
    assert_eq!(
        visible(&mut s, answer("q1", vec![one("Red"), one("S")]), now).len(),
        1
    );
}

#[test]
fn a_reused_request_id_sends_both_cards_to_the_terminal() {
    let now = Instant::now();
    let mut s = State::default();
    visible(&mut s, requested("a", "r1"), now);
    let effects = reduce(&mut s, requested("a", "r1"), now);
    assert_eq!(effects.last(), Some(&Effect::ReleasePermission(rid("r1"))));
    assert!(!effects.iter().any(|e| matches!(e, Effect::AckPermission(_))));
    assert_eq!(
        audits(&effects),
        vec![(
            audit::Actor::System,
            audit::Act::Release,
            "Bash · cargo test".into()
        )]
    );
    // Nothing is left a click could answer.
    assert!(s.pending.is_empty() && !s.ledger.is_open(&rid("r1")));
    assert!(visible(&mut s, decide("r1", Decision::Allow), now).is_empty());
}

/// Random sequences of agent events, clicks, presets and time against `reduce`'s invariants
/// (`docs/dev/plan-zeca.md` S4; road-to-0.2 section 2; rule 2). A failure prints the shortest
/// sequence that breaks one: paste it into `check(&[...])` to replay it. `PROPTEST_CASES=20000`
/// runs deeper.
mod properties {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap as Map;

    const FOLDERS: [Option<&str>; 3] = [Some("/home/me/vults"), Some("/home/me/site"), None];
    const AGENTS: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];
    const TARGETS: [(&str, &str); 3] = [
        ("Bash", "Bash · cargo test"),
        ("Bash", "Bash · rm -rf x"),
        ("Read", "src/main.rs"),
    ];

    /// Who sends an event: session, which agent it is, its folder, and a subagent maybe.
    #[derive(Clone, Debug)]
    struct From {
        session: u8,
        agent: u8,
        folder: u8,
        sub: Option<u8>,
    }

    #[derive(Clone, Debug)]
    enum Step {
        Ask {
            from: From,
            id: u8,
            target: u8,
            cut: bool,
        },
        Question {
            from: From,
            id: u8,
        },
        Event {
            from: From,
            kind: u8,
        },
        Decide {
            id: u8,
            allow: bool,
        },
        Always {
            id: u8,
        },
        /// 0: every reply, 1: one reply short, 2: none, 3: a blank reply.
        Answer {
            id: u8,
            shape: u8,
        },
        Release {
            id: u8,
        },
        Focus {
            session: u8,
        },
        Presence(u8),
        /// Which of the candidate rules are saved, as bits.
        Rules(u8),
        Locked(bool),
        Tick,
    }

    fn from() -> impl Strategy<Value = From> {
        (0..3u8, 0..2u8, 0..3u8, prop::option::of(0..2u8)).prop_map(|(session, agent, folder, sub)| From {
            session,
            agent,
            folder,
            sub,
        })
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            4 => (from(), 0..6u8, 0..3u8, any::<bool>())
                .prop_map(|(from, id, target, cut)| Step::Ask { from, id, target, cut }),
            2 => (from(), 0..6u8).prop_map(|(from, id)| Step::Question { from, id }),
            3 => (from(), 0..5u8).prop_map(|(from, kind)| Step::Event { from, kind }),
            4 => (0..6u8, any::<bool>()).prop_map(|(id, allow)| Step::Decide { id, allow }),
            2 => (0..6u8).prop_map(|id| Step::Always { id }),
            2 => (0..6u8, 0..4u8).prop_map(|(id, shape)| Step::Answer { id, shape }),
            1 => (0..6u8).prop_map(|id| Step::Release { id }),
            1 => (0..3u8).prop_map(|session| Step::Focus { session }),
            1 => (0..4u8).prop_map(Step::Presence),
            1 => any::<u8>().prop_map(Step::Rules),
            1 => any::<bool>().prop_map(Step::Locked),
            2 => Just(Step::Tick),
        ]
    }

    fn id(n: u8) -> RequestId {
        rid(&format!("r{n}"))
    }

    fn sender(f: &From) -> SessionKey {
        SessionKey {
            agent: AGENTS[f.agent as usize],
            session_id: ["a", "b", "c"][f.session as usize].into(),
        }
    }

    fn update(f: &From, event: AgentEvent) -> Input {
        Input::Agent(AgentUpdate {
            session: sender(f),
            cwd: FOLDERS[f.folder as usize].map(str::to_owned),
            terminal: Terminal::default(),
            agent_id: f.sub.map(|n| format!("sub-{n}")),
            event,
        })
    }

    /// Eight candidate rules: two agents, two folders, two targets.
    fn rules(bits: u8) -> Vec<Rule> {
        (0..8)
            .filter(|i| bits & (1 << i) != 0)
            .map(|i| Rule {
                agent: AGENTS[i & 1],
                cwd: FOLDERS[(i >> 1) & 1].unwrap_or_default().to_owned(),
                tool: "Bash".into(),
                target: TARGETS[(i >> 2) & 1].1.into(),
            })
            .collect()
    }

    fn input(step: &Step) -> Input {
        match step.clone() {
            Step::Ask {
                from,
                id: n,
                target,
                cut,
            } => {
                let (tool, target) = TARGETS[target as usize];
                update(
                    &from,
                    AgentEvent::PermissionRequested {
                        request: id(n),
                        tool: tool.into(),
                        target: target.into(),
                        ask: Ask {
                            cut,
                            ..Ask::default()
                        },
                    },
                )
            }
            Step::Question { from, id: n } => {
                let Input::Agent(u) = asked("a", "x") else {
                    unreachable!()
                };
                let AgentEvent::QuestionAsked {
                    target, questions, ..
                } = u.event
                else {
                    unreachable!()
                };
                update(
                    &from,
                    AgentEvent::QuestionAsked {
                        request: id(n),
                        target,
                        questions,
                    },
                )
            }
            Step::Event { from, kind } => update(
                &from,
                match kind {
                    0 => AgentEvent::PromptSubmitted,
                    1 => AgentEvent::ToolStarted(super::Step {
                        activity: Activity::Run,
                        tool: "Bash".into(),
                        detail: Some("cargo test".into()),
                    }),
                    2 => AgentEvent::ToolFinished {
                        failed: false,
                        target: Some("Bash · cargo test".into()),
                        diff: None,
                    },
                    3 => AgentEvent::Stopped { message: None },
                    _ => AgentEvent::SessionEnded,
                },
            ),
            Step::Decide { id: n, allow } => Input::User(Intent::Decide {
                request: id(n),
                decision: if allow { Decision::Allow } else { Decision::Deny },
            }),
            Step::Always { id: n } => Input::User(Intent::DecideAlways { request: id(n) }),
            Step::Answer { id: n, shape } => Input::User(Intent::Answer {
                request: id(n),
                answers: match shape {
                    0 => vec![one("Red"), one("S")],
                    1 => vec![one("Red")],
                    2 => vec![],
                    _ => vec![one("Red"), one("  ")],
                },
            }),
            Step::Release { id: n } => Input::User(Intent::Release { request: id(n) }),
            Step::Focus { session } => Input::User(Intent::Focus {
                session: Some(key(["a", "b", "c"][session as usize])),
            }),
            Step::Presence(p) => Input::SetPresence(
                [
                    Presence::Island,
                    Presence::Panel,
                    Presence::Quiet,
                    Presence::Paused,
                ][p as usize],
            ),
            Step::Rules(bits) => Input::SetRules(rules(bits)),
            Step::Locked(locked) => Input::Locked {
                locked,
                missed: Vec::new(),
            },
            Step::Tick => Input::Tick,
        }
    }

    /// A rule may answer this request at once: not paused, not cut, not a reused id, and one of the
    /// user's rules matches its agent, folder, tool and target exactly.
    fn rule_may_answer(s: &State, input: &Input) -> bool {
        let Input::Agent(u) = input else { return false };
        let AgentEvent::PermissionRequested {
            request,
            tool,
            target,
            ask,
        } = &u.event
        else {
            return false;
        };
        let cwd = u
            .cwd
            .clone()
            .filter(|c| project_name(c).is_some())
            .or_else(|| s.sessions.get(&u.session).and_then(|x| x.cwd.clone()));
        s.presence != Presence::Paused
            && !ask.cut
            && !s.pending.iter().any(|p| &p.request == request)
            && cwd.is_some_and(|cwd| {
                s.rules.iter().any(|r| {
                    r.agent == u.session.agent && r.cwd == cwd && &r.tool == tool && &r.target == target
                })
            })
    }

    /// Runs the steps, each `secs` after the last, checking every invariant after each one.
    fn check(steps: &[(Step, u64)]) -> Result<(), TestCaseError> {
        let mut now = Instant::now();
        let mut s = State::default();
        // Acknowledged and not yet settled, with when they were acknowledged.
        let mut open: Map<RequestId, Instant> = Map::new();
        for (step, secs) in steps {
            now += Duration::from_secs(*secs);
            let input = input(step);
            let paused = s.presence == Presence::Paused;
            let rule_ok = rule_may_answer(&s, &input);
            // The cards as they were before this step: what a click could have been aimed at.
            let before: Map<RequestId, Pending> =
                s.pending.iter().map(|p| (p.request.clone(), p.clone())).collect();
            let folders: Map<SessionKey, Option<String>> = s
                .sessions
                .iter()
                .map(|(k, x)| (k.clone(), x.cwd.clone()))
                .collect();
            let effects = reduce(&mut s, input.clone(), now);
            let mut answers = 0;
            let mut lines = 0;
            for e in &effects {
                match e {
                    Effect::AckPermission(r) => {
                        // D5, ADR 0009: Paused never acknowledges a card.
                        prop_assert!(!paused, "acknowledged while paused: {step:?}");
                        open.insert(r.clone(), now);
                    }
                    Effect::RespondPermission { request, .. } => {
                        answers += 1;
                        let since = open.remove(request);
                        prop_assert!(since.is_some(), "answered twice or never asked: {request:?}");
                        match &input {
                            // Rule 2: a rule answers only what it matches exactly.
                            Input::Agent(_) => {
                                prop_assert!(rule_ok, "a rule answered what it must not: {input:?}")
                            }
                            Input::User(
                                Intent::Decide { request: clicked, .. }
                                | Intent::DecideAlways { request: clicked },
                            ) => {
                                let card = before.get(request);
                                prop_assert!(card.is_some(), "answered a card that was not waiting");
                                let card = card.map(|c| (c.ask.cut, c.questions.is_empty()));
                                // A click (or the rule it saved) answers only a whole permission.
                                prop_assert_eq!(
                                    card,
                                    Some((false, true)),
                                    "answered a cut card or a question"
                                );
                                if clicked == request {
                                    prop_assert!(
                                        now.duration_since(since.unwrap_or(now)) < PENDING_TTL,
                                        "late answer"
                                    );
                                } else if let (Some(c), Some(a)) = (before.get(clicked), before.get(request))
                                {
                                    // The rule Always saved answers only the very same thing, in the same
                                    // folder (and only when that folder is known).
                                    let same = c.session.agent == a.session.agent
                                        && c.tool == a.tool
                                        && c.target == a.target
                                        && folders.get(&c.session).cloned().flatten().is_some()
                                        && folders.get(&c.session) == folders.get(&a.session);
                                    prop_assert!(same, "Always answered something else: {request:?}");
                                }
                            }
                            other => prop_assert!(false, "answered by {other:?}"),
                        }
                    }
                    Effect::AnswerQuestion {
                        request,
                        answers: given,
                    } => {
                        answers += 1;
                        prop_assert!(
                            matches!(input, Input::User(Intent::Answer { .. })),
                            "answered by {input:?}"
                        );
                        prop_assert!(
                            open.remove(request).is_some(),
                            "question answered twice: {request:?}"
                        );
                        let fit = before.get(request).is_some_and(|c| fits(&c.questions, given));
                        prop_assert!(fit, "a question answered with replies that do not fit");
                    }
                    Effect::ReleasePermission(r) => {
                        open.remove(r);
                        // An agent's later event settles only its own cards: a subagent's stay.
                        if let Input::Agent(u) = &input
                            && !matches!(
                                u.event,
                                AgentEvent::PermissionRequested { .. } | AgentEvent::QuestionAsked { .. }
                            )
                            && let Some(card) = before.get(r)
                        {
                            let ended = matches!(u.event, AgentEvent::SessionEnded);
                            prop_assert!(
                                card.session == u.session && (ended || card.agent_id == u.agent_id),
                                "an event settled another agent's card: {r:?}"
                            );
                        }
                    }
                    Effect::Audit(a)
                        if matches!(
                            a.act,
                            audit::Act::Allow
                                | audit::Act::Deny
                                | audit::Act::AlwaysAllow
                                | audit::Act::Answer
                        ) =>
                    {
                        lines += 1;
                    }
                    _ => {}
                }
            }
            // Every answer is on record.
            prop_assert_eq!(answers, lines);
            // The ledger holds exactly the cards on the line.
            let mut pending: Vec<&RequestId> = s.pending.iter().map(|p| &p.request).collect();
            let mut offers: Vec<&RequestId> = s.ledger.open().collect();
            pending.sort();
            offers.sort();
            prop_assert_eq!(pending, offers);
            // A waiting card is always one a hook waits on.
            for p in &s.pending {
                prop_assert!(
                    open.contains_key(&p.request),
                    "a card no hook waits on: {:?}",
                    p.request
                );
            }
            // D5: while paused, no hook waits.
            if s.presence == Presence::Paused {
                prop_assert!(open.is_empty(), "paused with a hook waiting");
            }
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        #[test]
        fn reduce_keeps_its_promises(steps in prop::collection::vec((step(), 0..70u64), 1..60)) {
            check(&steps)?;
        }
    }
}

#[test]
fn a_rule_never_answers_a_reused_id_while_its_old_card_waits() {
    // Found by the properties: the first request was cut (never ruled), the second, same id, was
    // ruled and answered while the old card stayed on the line.
    let now = Instant::now();
    let mut s = State::default();
    let rule = Rule {
        agent: AgentKind::Claude,
        cwd: "/home/me/vults".into(),
        tool: "Bash".into(),
        target: "Bash · cargo test".into(),
    };
    visible(&mut s, Input::SetRules(vec![rule]), now);
    let cut = agent(
        "a",
        AgentEvent::PermissionRequested {
            request: rid("r1"),
            tool: "Bash".into(),
            target: "Bash · cargo test".into(),
            ask: Ask {
                cut: true,
                ..Ask::default()
            },
        },
    );
    visible(&mut s, cut, now);
    let effects = visible(&mut s, requested("a", "r1"), now);
    assert_eq!(effects, vec![Effect::ReleasePermission(rid("r1"))]);
    assert!(s.pending.is_empty() && !s.ledger.is_open(&rid("r1")));
    assert!(visible(&mut s, decide("r1", Decision::Allow), now).is_empty());
}

#[test]
fn always_never_answers_a_cut_card_waiting_with_the_same_target() {
    // Found by the properties: Always on a whole card also answered an identical but cut one.
    let now = Instant::now();
    let mut s = State::default();
    let cut = agent(
        "a",
        AgentEvent::PermissionRequested {
            request: rid("r2"),
            tool: "Bash".into(),
            target: "Bash · cargo test".into(),
            ask: Ask {
                cut: true,
                ..Ask::default()
            },
        },
    );
    visible(&mut s, cut, now);
    visible(&mut s, requested("a", "r1"), now);
    let effects = visible(
        &mut s,
        Input::User(Intent::DecideAlways { request: rid("r1") }),
        now,
    );
    assert!(!effects.contains(&Effect::RespondPermission {
        request: rid("r2"),
        decision: Decision::Allow
    }));
    assert!(
        s.ledger.is_open(&rid("r2")),
        "the cut card still waits for its own click"
    );
}

#[test]
fn always_answers_the_same_request_only_in_the_same_folder() {
    let now = Instant::now();
    let mut s = State::default();
    visible(&mut s, requested("a", "r1"), now);
    // The same command, waiting in another project.
    let elsewhere = Input::Agent(AgentUpdate {
        session: key("b"),
        cwd: Some("/home/me/site".into()),
        terminal: Terminal::default(),
        agent_id: None,
        event: requested_event("r2"),
    });
    visible(&mut s, elsewhere, now);
    let effects = visible(
        &mut s,
        Input::User(Intent::DecideAlways { request: rid("r1") }),
        now,
    );
    assert!(!effects.contains(&Effect::RespondPermission {
        request: rid("r2"),
        decision: Decision::Allow
    }));
    assert!(s.ledger.is_open(&rid("r2")));
}
