//! Every card is an offer, and every answer redeems one (ADR 0014 and 0017, `docs/dev/plan-zeca.md`
//! S3). An answer counts only for the exact request it was made for (the binding: a hash of the whole
//! target), only once, only while the hook still waits, and from a paired device only with a counter
//! it never used before. The island's clicks go through here today; a paired phone will tomorrow.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::{Pending, RequestId};

/// An offer lives no longer than this, and never longer than the hook waits.
pub const MAX_LIFE: Duration = Duration::from_secs(120);

/// SHA-256 of everything the card asks about: a different command with the same id never matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Binding(pub [u8; 32]);

impl Binding {
    /// The binding of a waiting card: only what the request itself says, which never changes while
    /// it waits (not the session's folder, which follows the agent's `cd`).
    pub fn of(p: &Pending) -> Self {
        // Every field and every list length is prefixed: no two different requests make the same
        // stream of bytes.
        let mut h = Sha256::new();
        let count = |h: &mut Sha256, n: usize| h.update((n as u64).to_le_bytes());
        let field = |h: &mut Sha256, s: &str| {
            count(h, s.len());
            h.update(s.as_bytes());
        };
        let maybe = |h: &mut Sha256, s: Option<&str>| match s {
            Some(s) => {
                h.update([1]);
                field(h, s);
            }
            None => h.update([0]),
        };
        field(&mut h, p.request.0.as_str());
        field(&mut h, p.session.agent.name());
        field(&mut h, &p.session.session_id);
        maybe(&mut h, p.agent_id.as_deref());
        field(&mut h, &p.tool);
        field(&mut h, &p.target);
        // The whole command when the card shows only its start.
        maybe(&mut h, p.ask.full.as_deref());
        maybe(&mut h, p.ask.description.as_deref());
        count(&mut h, p.questions.len());
        for q in &p.questions {
            field(&mut h, &q.question);
            field(&mut h, &q.header);
            h.update([u8::from(q.multi)]);
            count(&mut h, q.options.len());
            for o in &q.options {
                field(&mut h, &o.label);
                maybe(&mut h, o.description.as_deref());
            }
        }
        Self(h.finalize().into())
    }
}

/// Where an answer comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Device {
    /// A click (or a key) on the island: on this machine, seen by the user.
    Island,
    /// A paired device (ADR 0017) and the counter it signed with: it must be higher than any the
    /// device used before.
    Paired { id: String, counter: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// No such card (never offered, or it left the line).
    Unknown,
    /// Its time ran out: the hook may already be gone.
    Expired,
    /// It was answered already.
    Used,
    /// The answer was made for a different request.
    Mismatch,
    /// A paired device's counter it already used: a replay.
    Replay,
}

#[derive(Clone, Debug)]
struct Offer {
    binding: Binding,
    expires: Instant,
    used: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Ledger {
    offers: BTreeMap<RequestId, Offer>,
    /// The highest counter each paired device has used.
    counters: BTreeMap<String, u64>,
}

impl Ledger {
    /// A card is on screen: its answer may come until `since + life`, `life` capped at
    /// [`MAX_LIFE`].
    pub fn offer(&mut self, request: RequestId, binding: Binding, since: Instant, life: Duration) {
        let expires = since + life.min(MAX_LIFE);
        self.offers.insert(
            request,
            Offer {
                binding,
                expires,
                used: false,
            },
        );
    }

    /// An answer for `request`, made for `binding`, from `device`, at `now`. On success the offer is
    /// used up; on a refusal nothing changes (a replayed counter is not taken either).
    pub fn redeem(
        &mut self,
        request: &RequestId,
        binding: Binding,
        device: &Device,
        now: Instant,
    ) -> Result<(), Refused> {
        let offer = self.offers.get_mut(request).ok_or(Refused::Unknown)?;
        if offer.used {
            return Err(Refused::Used);
        }
        if now >= offer.expires {
            return Err(Refused::Expired);
        }
        if offer.binding != binding {
            return Err(Refused::Mismatch);
        }
        if let Device::Paired { id, counter } = device {
            if self.counters.get(id).is_some_and(|last| counter <= last) {
                return Err(Refused::Replay);
            }
            self.counters.insert(id.clone(), *counter);
        }
        offer.used = true;
        Ok(())
    }

    /// The card left the line some other way (the terminal, a rule, its time): nothing may answer it.
    pub fn withdraw(&mut self, request: &RequestId) {
        self.offers.remove(request);
    }

    /// Forgets used and expired offers: a used one past its time can't be redeemed anyway.
    pub fn sweep(&mut self, now: Instant) {
        self.offers.retain(|_, o| now < o.expires);
    }

    /// Every request whose offer is still open.
    pub fn open(&self) -> impl Iterator<Item = &RequestId> {
        self.offers.iter().filter(|(_, o)| !o.used).map(|(r, _)| r)
    }

    pub fn is_open(&self, request: &RequestId) -> bool {
        self.offers.get(request).is_some_and(|o| !o.used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u8) -> Binding {
        Binding([n; 32])
    }

    fn rid(s: &str) -> RequestId {
        RequestId(s.into())
    }

    const LIFE: Duration = Duration::from_secs(108);

    #[test]
    fn an_offer_is_redeemed_once() {
        let now = Instant::now();
        let mut l = Ledger::default();
        l.offer(rid("r1"), b(1), now, LIFE);
        assert_eq!(l.redeem(&rid("r1"), b(1), &Device::Island, now), Ok(()));
        assert_eq!(
            l.redeem(&rid("r1"), b(1), &Device::Island, now),
            Err(Refused::Used)
        );
    }

    #[test]
    fn a_late_answer_is_refused() {
        let now = Instant::now();
        let mut l = Ledger::default();
        l.offer(rid("r1"), b(1), now, LIFE);
        assert_eq!(
            l.redeem(&rid("r1"), b(1), &Device::Island, now + LIFE),
            Err(Refused::Expired)
        );
        // Never longer than two minutes, whatever the hook says.
        l.offer(rid("r2"), b(2), now, Duration::from_secs(600));
        assert_eq!(
            l.redeem(&rid("r2"), b(2), &Device::Island, now + MAX_LIFE),
            Err(Refused::Expired)
        );
    }

    #[test]
    fn an_answer_for_something_else_is_refused() {
        let now = Instant::now();
        let mut l = Ledger::default();
        l.offer(rid("r1"), b(1), now, LIFE);
        assert_eq!(
            l.redeem(&rid("r1"), b(9), &Device::Island, now),
            Err(Refused::Mismatch)
        );
        assert_eq!(
            l.redeem(&rid("nope"), b(1), &Device::Island, now),
            Err(Refused::Unknown)
        );
        // Still open for the right answer.
        assert!(l.is_open(&rid("r1")));
    }

    #[test]
    fn a_paired_device_never_reuses_a_counter() {
        let now = Instant::now();
        let mut l = Ledger::default();
        let phone = |counter| Device::Paired {
            id: "phone".into(),
            counter,
        };
        l.offer(rid("r1"), b(1), now, LIFE);
        l.offer(rid("r2"), b(2), now, LIFE);
        l.offer(rid("r3"), b(3), now, LIFE);
        assert_eq!(l.redeem(&rid("r1"), b(1), &phone(5), now), Ok(()));
        assert_eq!(l.redeem(&rid("r2"), b(2), &phone(5), now), Err(Refused::Replay));
        assert_eq!(l.redeem(&rid("r2"), b(2), &phone(4), now), Err(Refused::Replay));
        assert!(l.is_open(&rid("r2")), "a replay takes nothing");
        assert_eq!(l.redeem(&rid("r2"), b(2), &phone(6), now), Ok(()));
        // Another device counts on its own.
        let tablet = Device::Paired {
            id: "tablet".into(),
            counter: 1,
        };
        assert_eq!(l.redeem(&rid("r3"), b(3), &tablet, now), Ok(()));
    }

    #[test]
    fn a_withdrawn_or_swept_offer_is_gone() {
        let now = Instant::now();
        let mut l = Ledger::default();
        l.offer(rid("r1"), b(1), now, LIFE);
        l.withdraw(&rid("r1"));
        assert_eq!(
            l.redeem(&rid("r1"), b(1), &Device::Island, now),
            Err(Refused::Unknown)
        );
        l.offer(rid("r2"), b(2), now, LIFE);
        l.sweep(now + LIFE);
        assert!(!l.is_open(&rid("r2")));
    }

    fn card(questions: Vec<crate::Question>) -> Pending {
        Pending {
            request: rid("q1"),
            session: crate::SessionKey {
                agent: vults_protocol::AgentKind::Claude,
                session_id: "s".into(),
            },
            agent_id: None,
            tool: "AskUserQuestion".into(),
            target: "AskUserQuestion".into(),
            ask: crate::Ask::default(),
            questions,
            since: Instant::now(),
            reminders: 0,
        }
    }

    fn q(question: &str, header: &str, multi: bool, options: &[&str]) -> crate::Question {
        crate::Question {
            question: question.into(),
            header: header.into(),
            options: options
                .iter()
                .map(|l| crate::Choice {
                    label: (*l).into(),
                    description: None,
                })
                .collect(),
            multi,
        }
    }

    #[test]
    fn different_question_sets_never_bind_the_same() {
        // One question whose options spell out a second question, against the two questions.
        let one = card(vec![q("Q", "H", false, &["x", "B", "H", "one"])]);
        let two = card(vec![q("Q", "H", false, &["x"]), q("B", "H", false, &[])]);
        assert_ne!(Binding::of(&one), Binding::of(&two));
        let mut described = one.clone();
        described.questions[0].options[0].description = Some("deletes everything".into());
        assert_ne!(Binding::of(&one), Binding::of(&described));
    }
}
