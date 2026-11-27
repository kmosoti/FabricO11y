//! Runtime-neutral operation lifecycle primitives.
//!
//! Adapters authenticate peers, enforce payload byte limits and supply monotonic
//! ticks. This crate neither parses wire claims nor performs I/O. A tick is only
//! meaningful in the adapter's local clock domain.

/// A monotonic tick and duration use the same adapter-defined unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tick(pub u64);

/// Identity established by an adapter, never deserialized by this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPeer {
    identity: Box<str>,
    locality: Locality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locality {
    Local,
    Remote,
}

impl VerifiedPeer {
    /// The caller must establish identity and locality at its trust boundary.
    pub fn from_adapter(identity: String, locality: Locality) -> Result<Self, Error> {
        if identity.is_empty() || identity.len() > 256 || identity.contains('\0') {
            return Err(Error::InvalidPeer);
        }
        Ok(Self {
            identity: identity.into_boxed_str(),
            locality,
        })
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn locality(&self) -> Locality {
        self.locality
    }
}

/// A local deadline is not a wire timestamp. It cannot be serialized here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallContext {
    peer: VerifiedPeer,
    started: Tick,
    deadline: Tick,
    wire_cap: u64,
}

impl CallContext {
    /// `requested` and `wire_cap` are durations, never remote clock readings.
    pub fn from_adapter(
        peer: VerifiedPeer,
        now: Tick,
        requested: u64,
        wire_cap: u64,
    ) -> Result<Self, Error> {
        if requested == 0 || wire_cap == 0 {
            return Err(Error::InvalidBudget);
        }
        let deadline = now.0.checked_add(requested).ok_or(Error::InvalidBudget)?;
        Ok(Self {
            peer,
            started: now,
            deadline: Tick(deadline),
            wire_cap,
        })
    }
    pub fn peer(&self) -> &VerifiedPeer {
        &self.peer
    }
    pub fn deadline(&self) -> Tick {
        self.deadline
    }
    /// The returned duration is safe to pass to another adapter, not an instant.
    pub fn wire_remaining(&self, now: Tick) -> Result<u64, Error> {
        if now < self.started {
            return Err(Error::ClockRegressed);
        }
        Ok(self.deadline.0.saturating_sub(now.0).min(self.wire_cap))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Unary,
    Notification,
    ServerStream,
    ClientStream,
    Bidi,
}

/// Bounds message counts and effects; adapters separately bound payload bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub input_messages: u32,
    pub output_messages: u32,
    pub effects_per_transition: usize,
}

pub const MAX_MESSAGES: u32 = 4096;
pub const MAX_EFFECTS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPeer,
    InvalidBudget,
    ClockRegressed,
    Deadline,
    InvalidLimits,
    InvalidSequence,
    MessageLimit,
    EffectLimit,
}

/// Input and output are concrete caller types; no erased transport payload is needed.
#[derive(Debug, PartialEq, Eq)]
pub enum Event<I, O> {
    Input(I),
    CloseInput,
    Output(O),
    /// Bidi input and output accepted atomically in one transition.
    Exchange(I, O),
    Finish,
    Cancel,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Effect<I, O> {
    Input(I),
    InputClosed,
    Output(O),
    /// Local lifecycle completion; this is not a delivery or custody receipt.
    Completed,
    Cancelled,
}

/// Fixed-capacity, non-allocating effects. The limit is checked before mutation.
#[derive(Debug, PartialEq, Eq)]
pub struct EffectBatch<I, O> {
    effects: [Option<Effect<I, O>>; MAX_EFFECTS],
    len: usize,
    limit: usize,
}
impl<I, O> EffectBatch<I, O> {
    fn new(limit: usize) -> Self {
        Self {
            effects: std::array::from_fn(|_| None),
            len: 0,
            limit,
        }
    }
    fn push(&mut self, effect: Effect<I, O>) -> Result<(), Error> {
        if self.len == self.limit {
            return Err(Error::EffectLimit);
        }
        self.effects[self.len] = Some(effect);
        self.len += 1;
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = &Effect<I, O>> {
        self.effects[..self.len].iter().flatten()
    }
    pub fn into_effects(self) -> impl Iterator<Item = Effect<I, O>> {
        self.effects.into_iter().flatten()
    }
}

/// One call's deterministic lifecycle; adapters drive all input and output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    shape: Shape,
    context: CallContext,
    limits: Limits,
    inputs: u32,
    outputs: u32,
    input_closed: bool,
    terminal: bool,
    last_tick: Tick,
}
impl Session {
    pub fn new(shape: Shape, context: CallContext, limits: Limits) -> Result<Self, Error> {
        if limits.input_messages == 0
            || limits.input_messages > MAX_MESSAGES
            || limits.output_messages > MAX_MESSAGES
            || limits.effects_per_transition == 0
            || limits.effects_per_transition > MAX_EFFECTS
        {
            return Err(Error::InvalidLimits);
        }
        if shape != Shape::Notification && limits.output_messages == 0 {
            return Err(Error::InvalidLimits);
        }
        let last_tick = context.started;
        Ok(Self {
            shape,
            context,
            limits,
            inputs: 0,
            outputs: 0,
            input_closed: false,
            terminal: false,
            last_tick,
        })
    }
    pub fn context(&self) -> &CallContext {
        &self.context
    }
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }
    pub fn counts(&self) -> (u32, u32) {
        (self.inputs, self.outputs)
    }

    /// On every error the session remains unchanged, including effect overflow.
    pub fn transition<I, O>(
        &mut self,
        now: Tick,
        event: Event<I, O>,
    ) -> Result<EffectBatch<I, O>, Error> {
        if self.terminal {
            return Err(Error::InvalidSequence);
        }
        if now < self.last_tick {
            return Err(Error::ClockRegressed);
        }
        let remaining = self.context.wire_remaining(now)?;
        if remaining == 0 && !matches!(event, Event::Cancel) {
            return Err(Error::Deadline);
        }
        let mut next = self.clone();
        next.last_tick = now;
        let mut effects = EffectBatch::new(self.limits.effects_per_transition);
        match event {
            Event::Input(value) => {
                if next.input_closed {
                    return Err(Error::InvalidSequence);
                }
                if matches!(
                    next.shape,
                    Shape::Unary | Shape::Notification | Shape::ServerStream
                ) && next.inputs != 0
                {
                    return Err(Error::InvalidSequence);
                }
                if next.inputs >= next.limits.input_messages {
                    return Err(Error::MessageLimit);
                }
                next.inputs += 1;
                effects.push(Effect::Input(value))?;
            }
            Event::CloseInput => {
                if next.input_closed
                    || (matches!(
                        next.shape,
                        Shape::Unary | Shape::Notification | Shape::ServerStream
                    ) && next.inputs != 1)
                {
                    return Err(Error::InvalidSequence);
                }
                next.input_closed = true;
                effects.push(Effect::InputClosed)?;
                if next.shape == Shape::Notification {
                    next.terminal = true;
                    effects.push(Effect::Completed)?;
                }
            }
            Event::Output(value) => {
                if next.shape == Shape::Notification
                    || (!next.input_closed && next.shape != Shape::Bidi)
                {
                    return Err(Error::InvalidSequence);
                }
                if matches!(next.shape, Shape::Unary | Shape::ClientStream) && next.outputs != 0 {
                    return Err(Error::InvalidSequence);
                }
                if next.outputs >= next.limits.output_messages {
                    return Err(Error::MessageLimit);
                }
                next.outputs += 1;
                effects.push(Effect::Output(value))?;
            }
            Event::Exchange(input, output) => {
                if next.shape != Shape::Bidi || next.input_closed {
                    return Err(Error::InvalidSequence);
                }
                if next.inputs >= next.limits.input_messages
                    || next.outputs >= next.limits.output_messages
                {
                    return Err(Error::MessageLimit);
                }
                next.inputs += 1;
                next.outputs += 1;
                effects.push(Effect::Input(input))?;
                effects.push(Effect::Output(output))?;
            }
            Event::Finish => {
                if !next.input_closed
                    || next.shape == Shape::Notification
                    || (matches!(next.shape, Shape::Unary | Shape::ClientStream)
                        && next.outputs != 1)
                {
                    return Err(Error::InvalidSequence);
                }
                next.terminal = true;
                effects.push(Effect::Completed)?;
            }
            Event::Cancel => {
                next.terminal = true;
                effects.push(Effect::Cancelled)?;
            }
        }
        *self = next;
        Ok(effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> CallContext {
        CallContext::from_adapter(
            VerifiedPeer::from_adapter("peer".into(), Locality::Remote).unwrap(),
            Tick(10),
            100,
            7,
        )
        .unwrap()
    }
    fn session(shape: Shape) -> Session {
        Session::new(
            shape,
            context(),
            Limits {
                input_messages: 2,
                output_messages: 2,
                effects_per_transition: 2,
            },
        )
        .unwrap()
    }
    fn step(s: &mut Session, event: Event<u8, u8>) -> Result<Vec<Effect<u8, u8>>, Error> {
        Ok(s.transition(Tick(11), event)?.into_effects().collect())
    }

    #[test]
    fn local_deadline_and_wire_budget_are_distinct_and_checked() {
        let c = context();
        assert_eq!(c.deadline(), Tick(110));
        assert_eq!(c.wire_remaining(Tick(10)), Ok(7));
        assert_eq!(c.wire_remaining(Tick(105)), Ok(5));
        assert_eq!(c.wire_remaining(Tick(110)), Ok(0));
        assert_eq!(c.wire_remaining(Tick(9)), Err(Error::ClockRegressed));
        assert_eq!(
            CallContext::from_adapter(c.peer().clone(), Tick(u64::MAX), 1, 1),
            Err(Error::InvalidBudget)
        );
        assert_eq!(
            CallContext::from_adapter(c.peer().clone(), Tick(0), 0, 1),
            Err(Error::InvalidBudget)
        );
        assert!(VerifiedPeer::from_adapter("".into(), Locality::Local).is_err());
        assert!(VerifiedPeer::from_adapter("a\0b".into(), Locality::Local).is_err());
        assert!(VerifiedPeer::from_adapter("x".repeat(257), Locality::Local).is_err());
        let mut monotonic = session(Shape::Bidi);
        monotonic
            .transition(Tick(20), Event::<u8, u8>::Input(1))
            .unwrap();
        let before = monotonic.clone();
        assert_eq!(
            monotonic.transition(Tick(19), Event::<u8, u8>::Input(2)),
            Err(Error::ClockRegressed)
        );
        assert_eq!(monotonic, before);
        let mut s = session(Shape::Bidi);
        assert_eq!(
            s.transition(Tick(110), Event::<u8, u8>::Input(1)),
            Err(Error::Deadline)
        );
        assert_eq!(
            s.transition(Tick(110), Event::<u8, u8>::Cancel)
                .unwrap()
                .into_effects()
                .collect::<Vec<_>>(),
            vec![Effect::Cancelled]
        );
    }

    #[test]
    fn five_shapes_enforce_their_lifecycles() {
        let mut unary = session(Shape::Unary);
        assert_eq!(
            step(&mut unary, Event::Output(1)),
            Err(Error::InvalidSequence)
        );
        assert_eq!(
            step(&mut unary, Event::Input(1)).unwrap(),
            vec![Effect::Input(1)]
        );
        assert_eq!(
            step(&mut unary, Event::Input(2)),
            Err(Error::InvalidSequence)
        );
        step(&mut unary, Event::CloseInput).unwrap();
        step(&mut unary, Event::Output(3)).unwrap();
        assert_eq!(
            step(&mut unary, Event::Output(4)),
            Err(Error::InvalidSequence)
        );
        assert_eq!(
            step(&mut unary, Event::Finish).unwrap(),
            vec![Effect::Completed]
        );
        assert_eq!(step(&mut unary, Event::Cancel), Err(Error::InvalidSequence));

        let mut notification = session(Shape::Notification);
        step(&mut notification, Event::Input(1)).unwrap();
        assert_eq!(
            step(&mut notification, Event::CloseInput).unwrap(),
            vec![Effect::InputClosed, Effect::Completed]
        );
        assert!(notification.is_terminal());

        let mut server = session(Shape::ServerStream);
        step(&mut server, Event::Input(1)).unwrap();
        step(&mut server, Event::CloseInput).unwrap();
        step(&mut server, Event::Output(1)).unwrap();
        step(&mut server, Event::Output(2)).unwrap();
        assert_eq!(
            step(&mut server, Event::Output(3)),
            Err(Error::MessageLimit)
        );
        step(&mut server, Event::Finish).unwrap();

        let mut client = session(Shape::ClientStream);
        step(&mut client, Event::Input(1)).unwrap();
        step(&mut client, Event::Input(2)).unwrap();
        assert_eq!(step(&mut client, Event::Input(3)), Err(Error::MessageLimit));
        step(&mut client, Event::CloseInput).unwrap();
        assert_eq!(
            step(&mut client, Event::Finish),
            Err(Error::InvalidSequence)
        );
        step(&mut client, Event::Output(1)).unwrap();
        step(&mut client, Event::Finish).unwrap();

        let mut bidi = session(Shape::Bidi);
        assert_eq!(
            step(&mut bidi, Event::Exchange(1, 1)).unwrap(),
            vec![Effect::Input(1), Effect::Output(1)]
        );
        step(&mut bidi, Event::CloseInput).unwrap();
        step(&mut bidi, Event::Output(2)).unwrap();
        step(&mut bidi, Event::Finish).unwrap();
    }

    #[test]
    fn exact_hard_message_limit_is_accepted_then_refused() {
        let mut s = Session::new(
            Shape::Bidi,
            context(),
            Limits {
                input_messages: MAX_MESSAGES,
                output_messages: MAX_MESSAGES,
                effects_per_transition: MAX_EFFECTS,
            },
        )
        .unwrap();
        for _ in 0..MAX_MESSAGES {
            step(&mut s, Event::Input(1)).unwrap();
        }
        let before = s.clone();
        assert_eq!(step(&mut s, Event::Input(2)), Err(Error::MessageLimit));
        assert_eq!(s, before);
        assert_eq!(
            step(&mut s, Event::Exchange(1, 1)),
            Err(Error::MessageLimit)
        );
        assert_eq!(s, before);
    }

    #[test]
    fn rejection_never_commits_state_or_partial_effects() {
        let mut bidi = Session::new(
            Shape::Bidi,
            context(),
            Limits {
                input_messages: 1,
                output_messages: 1,
                effects_per_transition: 1,
            },
        )
        .unwrap();
        let unchanged = bidi.clone();
        assert_eq!(
            step(&mut bidi, Event::Exchange(1, 2)),
            Err(Error::EffectLimit)
        );
        assert_eq!(bidi, unchanged);

        let mut s = Session::new(
            Shape::Notification,
            context(),
            Limits {
                input_messages: 1,
                output_messages: 0,
                effects_per_transition: 1,
            },
        )
        .unwrap();
        step(&mut s, Event::Input(1)).unwrap();
        let before = s.clone();
        assert_eq!(step(&mut s, Event::CloseInput), Err(Error::EffectLimit));
        assert_eq!(s, before);
        assert_eq!(
            s.transition(Tick(9), Event::<u8, u8>::Cancel),
            Err(Error::ClockRegressed)
        );
        assert_eq!(s, before);
        assert_eq!(
            s.transition(Tick(110), Event::<u8, u8>::CloseInput),
            Err(Error::Deadline)
        );
        assert_eq!(s, before);
        assert_eq!(
            step(&mut s, Event::Cancel).unwrap(),
            vec![Effect::Cancelled]
        );
        for limits in [
            Limits {
                input_messages: 0,
                output_messages: 1,
                effects_per_transition: 1,
            },
            Limits {
                input_messages: MAX_MESSAGES + 1,
                output_messages: 1,
                effects_per_transition: 1,
            },
            Limits {
                input_messages: 1,
                output_messages: 1,
                effects_per_transition: MAX_EFFECTS + 1,
            },
            Limits {
                input_messages: 1,
                output_messages: 0,
                effects_per_transition: 1,
            },
        ] {
            assert_eq!(
                Session::new(Shape::Bidi, context(), limits),
                Err(Error::InvalidLimits)
            );
        }
    }
}
