//! Deterministic packet-slot model for the registered H1 and M2 cells.
//! This is experimental network scheduling tooling, not a transport service.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Variant {
    M0,
    M1,
    M2,
}

impl Variant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::M0 => "M0",
            Self::M1 => "M1",
            Self::M2 => "M2",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub producers: usize,
    pub bursts: usize,
    pub message_bytes: usize,
    pub packet_size: usize,
    pub data_delay_ticks: u64,
    pub control_delay_ticks: u64,
    pub bdp_packets: usize,
    pub unscheduled_prefix_packets: usize,
    pub sender_queue_cap_bytes: usize,
    pub switch_queue_cap_bytes: usize,
    pub injection_interval_ticks: u64,
    pub drain_deadline_ticks: u64,
    pub nominal_burst_spacing_ticks: u64,
    pub jitter_magnitude_ticks: i64,
}

impl Config {
    pub fn h1() -> Self {
        Self {
            producers: 32,
            bursts: 1_000,
            message_bytes: 65_536,
            packet_size: 1_500,
            data_delay_ticks: 4,
            control_delay_ticks: 4,
            bdp_packets: 9,
            unscheduled_prefix_packets: 0,
            sender_queue_cap_bytes: 131_072,
            switch_queue_cap_bytes: 512 * 1_500,
            injection_interval_ticks: 1_760_000,
            drain_deadline_ticks: 1_770_000,
            nominal_burst_spacing_ticks: 1_760,
            jitter_magnitude_ticks: 400,
        }
    }

    pub fn m2_small() -> Self {
        Self {
            producers: 8,
            bursts: 1_000,
            message_bytes: 148,
            packet_size: 1_500,
            data_delay_ticks: 4,
            control_delay_ticks: 4,
            bdp_packets: 9,
            unscheduled_prefix_packets: 1,
            sender_queue_cap_bytes: 592,
            switch_queue_cap_bytes: 512 * 1_500,
            injection_interval_ticks: 10_000,
            drain_deadline_ticks: 11_000,
            nominal_burst_spacing_ticks: 10,
            jitter_magnitude_ticks: 3,
        }
    }

    pub fn m2_incast() -> Self {
        let mut config = Self::h1();
        config.unscheduled_prefix_packets = 1;
        config
    }

    fn validate(&self) -> Result<(), String> {
        if self.producers == 0
            || self.bursts == 0
            || self.message_bytes == 0
            || self.packet_size == 0
            || self.bdp_packets == 0
            || self.data_delay_ticks == 0
            || self.control_delay_ticks == 0
        {
            return Err("all counts, sizes, budgets, and delays must be positive".into());
        }
        if self.jitter_magnitude_ticks < 0
            || self.nominal_burst_spacing_ticks <= 2 * self.jitter_magnitude_ticks as u64
        {
            return Err("burst spacing must exceed twice the jitter bound".into());
        }
        if self.drain_deadline_ticks < self.injection_interval_ticks {
            return Err("drain deadline precedes injection end".into());
        }
        if self.message_bytes > self.sender_queue_cap_bytes {
            return Err("one message exceeds sender queue cap".into());
        }
        Ok(())
    }

    pub fn packets_per_message(&self) -> usize {
        self.message_bytes.div_ceil(self.packet_size)
    }

    fn packets_for(&self, variant: Variant) -> usize {
        if variant == Variant::M2 {
            1 + self
                .message_bytes
                .saturating_sub(self.packet_size - EMBEDDED_ANNOUNCEMENT_BYTES)
                .div_ceil(self.packet_size)
        } else {
            self.packets_per_message()
        }
    }

    fn packet_payload_bytes(&self, variant: Variant, packet_index: usize) -> usize {
        if variant == Variant::M2 {
            let first_payload = self.packet_size - EMBEDDED_ANNOUNCEMENT_BYTES;
            if packet_index == 0 {
                self.message_bytes.min(first_payload)
            } else {
                let offset = first_payload + (packet_index - 1) * self.packet_size;
                (self.message_bytes - offset).min(self.packet_size)
            }
        } else {
            let offset = packet_index * self.packet_size;
            (self.message_bytes - offset).min(self.packet_size)
        }
    }
}

const EMBEDDED_ANNOUNCEMENT_BYTES: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageSpec {
    pub producer: usize,
    pub sequence: usize,
    pub burst: usize,
    pub bytes: usize,
    pub release_tick: u64,
}

#[derive(Clone, Debug)]
pub struct Trace {
    pub releases: Vec<u64>,
    pub messages: Vec<MessageSpec>,
}

impl Trace {
    pub fn generate(config: &Config, seed: u64) -> Result<Self, String> {
        config.validate()?;
        let mut state = seed;
        let mut releases = Vec::with_capacity(config.bursts);
        let mut messages = Vec::with_capacity(config.producers * config.bursts);
        for burst in 0..config.bursts {
            let nominal = burst as u64 * config.nominal_burst_spacing_ticks;
            let release = if burst == 0 {
                0
            } else {
                let width = (2 * config.jitter_magnitude_ticks + 1) as u64;
                let jitter =
                    (splitmix64_next(&mut state) % width) as i64 - config.jitter_magnitude_ticks;
                nominal
                    .checked_add_signed(jitter)
                    .ok_or("release tick overflow")?
            };
            if release >= config.injection_interval_ticks {
                return Err(format!("burst {burst} releases after injection interval"));
            }
            releases.push(release);
            for producer in 0..config.producers {
                messages.push(MessageSpec {
                    producer,
                    sequence: burst + 1,
                    burst,
                    bytes: config.message_bytes,
                    release_tick: release,
                });
            }
        }
        Ok(Self { releases, messages })
    }
}

pub fn splitmix64_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[derive(Clone, Debug, Default)]
pub struct MessageResult {
    pub first_send_tick: Option<u64>,
    pub complete_tick: Option<u64>,
    pub modeled_commit_tick: Option<u64>,
    pub ack_tick: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct MessageState {
    sent_packets: usize,
    granted_packets: usize,
    delivered_packets: usize,
    delivered_bytes: usize,
    available_credits: usize,
    announced: bool,
    retained: bool,
    result: MessageResult,
}

#[derive(Clone, Debug, Default)]
pub struct BurstResult {
    pub last_complete_tick: Option<u64>,
    pub peak_hotspot_bytes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub completed_injection_messages: usize,
    pub completed_injection_bytes: u64,
    pub completed_deadline_messages: usize,
    pub ack_count: usize,
    pub rejection_count: usize,
    pub loss_count: usize,
    pub control_messages: u64,
    pub control_bytes: u64,
    pub data_packets_sent: u64,
    pub unscheduled_packets_sent: u64,
    pub scheduled_packets_sent: u64,
    pub embedded_metadata_bytes: u64,
    pub data_wire_bytes_sent: u64,
    pub data_packets_delivered: u64,
    pub modeled_commits: usize,
    pub sender_peak_bytes: usize,
    pub max_sender_queue_per_producer_bytes: usize,
    pub sender_retained_peak_bytes: usize,
    pub max_sender_retained_per_producer_bytes: usize,
    pub sender_byte_ticks: u128,
    pub switch_peak_bytes: usize,
    pub switch_byte_ticks: u128,
    pub receiver_peak_bytes: usize,
    pub receiver_byte_ticks: u128,
    pub total_queue_peak_bytes: usize,
    pub total_queue_byte_ticks: u128,
    pub p99_burst_peak_hotspot_bytes: usize,
    pub max_granted_outstanding_packets: usize,
    pub max_sender_window_outstanding_packets: usize,
    pub cap_overflow_count: usize,
    pub unacked_at_deadline: usize,
}

#[derive(Clone, Debug)]
pub struct Run {
    pub seed: u64,
    pub variant: Variant,
    pub config: Config,
    pub trace: Trace,
    pub messages: Vec<MessageResult>,
    pub bursts: Vec<BurstResult>,
    pub summary: Summary,
}

#[derive(Clone, Copy, Debug)]
struct Packet {
    message: usize,
    payload_bytes: usize,
    wire_bytes: usize,
    scheduled: bool,
}

#[derive(Clone, Copy, Debug)]
enum Event {
    Announce(usize),
    Grant(usize),
    DataToSwitch(Packet),
    ReceiverDelivery(Packet),
    PacketReceiptAck(usize),
    DurableAck(usize),
}

fn check_credit_budget(outstanding: usize, budget: usize) -> Result<(), String> {
    if outstanding > budget {
        Err(format!(
            "receiver credit invariant violated: {outstanding} outstanding grants > budget {budget}"
        ))
    } else {
        Ok(())
    }
}

struct Simulator<'a> {
    config: &'a Config,
    trace: &'a Trace,
    variant: Variant,
    states: Vec<MessageState>,
    bursts: Vec<BurstResult>,
    burst_remaining: Vec<usize>,
    active_bursts: Vec<usize>,
    producer_messages: Vec<Vec<usize>>,
    send_cursor: Vec<usize>,
    grant_cursor: Vec<usize>,
    next_grant_producer: usize,
    sender_queued: Vec<usize>,
    sender_retained: Vec<usize>,
    sender_window_outstanding: Vec<usize>,
    total_sender_queued: usize,
    total_sender_retained: usize,
    grants_outstanding: usize,
    switch: VecDeque<Packet>,
    switch_queued_bytes: usize,
    events: Vec<Vec<Event>>,
    summary: Summary,
}

impl<'a> Simulator<'a> {
    fn new(config: &'a Config, trace: &'a Trace, variant: Variant) -> Self {
        let mut producer_messages = vec![Vec::with_capacity(config.bursts); config.producers];
        for (index, spec) in trace.messages.iter().enumerate() {
            producer_messages[spec.producer].push(index);
        }
        Self {
            config,
            trace,
            variant,
            states: vec![MessageState::default(); trace.messages.len()],
            bursts: vec![BurstResult::default(); config.bursts],
            burst_remaining: vec![config.producers; config.bursts],
            active_bursts: Vec::new(),
            producer_messages,
            send_cursor: vec![0; config.producers],
            grant_cursor: vec![0; config.producers],
            next_grant_producer: 0,
            sender_queued: vec![0; config.producers],
            sender_retained: vec![0; config.producers],
            sender_window_outstanding: vec![0; config.producers],
            total_sender_queued: 0,
            total_sender_retained: 0,
            grants_outstanding: 0,
            switch: VecDeque::new(),
            switch_queued_bytes: 0,
            events: vec![Vec::new(); 16],
            summary: Summary::default(),
        }
    }

    fn schedule(&mut self, tick: u64, event: Event) {
        let slot = tick as usize % self.events.len();
        self.events[slot].push(event);
    }

    fn control(&mut self) {
        self.summary.control_messages += 1;
        self.summary.control_bytes += 32;
    }

    fn release_burst(&mut self, burst: usize, tick: u64) -> Result<(), String> {
        self.active_bursts.push(burst);
        for producer in 0..self.config.producers {
            let message = burst * self.config.producers + producer;
            let bytes = self.trace.messages[message].bytes;
            let new_queued = self.sender_queued[producer] + bytes;
            if new_queued > self.config.sender_queue_cap_bytes {
                self.summary.cap_overflow_count += 1;
                return Err(format!(
                    "sender queue cap overflow at tick {tick}, producer {producer}: {new_queued} > {}",
                    self.config.sender_queue_cap_bytes
                ));
            }
            self.sender_queued[producer] = new_queued;
            self.total_sender_queued += bytes;
            let new_retained = self.sender_retained[producer] + bytes;
            if new_retained > self.config.sender_queue_cap_bytes {
                self.summary.cap_overflow_count += 1;
                return Err(format!(
                    "sender retained-copy cap overflow at tick {tick}, producer {producer}: {new_retained} > {}",
                    self.config.sender_queue_cap_bytes
                ));
            }
            self.sender_retained[producer] = new_retained;
            self.total_sender_retained += bytes;
            self.states[message].retained = true;
            if self.variant == Variant::M1 {
                self.control();
                self.schedule(
                    tick + self.config.control_delay_ticks,
                    Event::Announce(message),
                );
            }
        }
        Ok(())
    }

    fn receive_event(&mut self, tick: u64, event: Event) -> Result<(), String> {
        match event {
            Event::Announce(message) => {
                self.states[message].announced = true;
            }
            Event::Grant(message) => {
                self.states[message].available_credits += 1;
            }
            Event::DataToSwitch(packet) => {
                let new_bytes = self.switch_queued_bytes + packet.wire_bytes;
                if new_bytes > self.config.switch_queue_cap_bytes {
                    self.summary.cap_overflow_count += 1;
                    return Err(format!(
                        "switch queue cap overflow at tick {tick}: {new_bytes} > {}",
                        self.config.switch_queue_cap_bytes
                    ));
                }
                self.switch.push_back(packet);
                self.switch_queued_bytes = new_bytes;
            }
            Event::ReceiverDelivery(packet) => {
                let state = &mut self.states[packet.message];
                state.delivered_packets += 1;
                state.delivered_bytes += packet.payload_bytes;
                self.summary.data_packets_delivered += 1;
                if packet.scheduled {
                    self.grants_outstanding = self
                        .grants_outstanding
                        .checked_sub(1)
                        .ok_or("delivered packet without an outstanding receiver grant")?;
                } else if self.variant == Variant::M0 {
                    self.control();
                    let producer = self.trace.messages[packet.message].producer;
                    self.schedule(
                        tick + self.config.control_delay_ticks,
                        Event::PacketReceiptAck(producer),
                    );
                } else if self.variant == Variant::M2 && state.delivered_packets == 1 {
                    // The embedded announcement becomes visible only at delivery.
                    state.announced = true;
                } else {
                    return Err("unexpected uncredited data packet delivery".into());
                }
                if self.states[packet.message].delivered_packets
                    == self.config.packets_for(self.variant)
                {
                    if self.states[packet.message].delivered_bytes != self.config.message_bytes {
                        return Err(format!(
                            "packet byte conservation failed for message {}",
                            packet.message
                        ));
                    }
                    self.states[packet.message].result.complete_tick = Some(tick);
                    self.states[packet.message].result.modeled_commit_tick = Some(tick);
                    self.summary.modeled_commits += 1;
                    if tick < self.config.injection_interval_ticks {
                        self.summary.completed_injection_messages += 1;
                        self.summary.completed_injection_bytes += self.config.message_bytes as u64;
                    }
                    let burst = self.trace.messages[packet.message].burst;
                    self.burst_remaining[burst] -= 1;
                    if self.burst_remaining[burst] == 0 {
                        self.bursts[burst].last_complete_tick = Some(tick);
                    }
                    self.control();
                    self.schedule(
                        tick + self.config.control_delay_ticks,
                        Event::DurableAck(packet.message),
                    );
                }
            }
            Event::PacketReceiptAck(producer) => {
                self.sender_window_outstanding[producer] = self.sender_window_outstanding[producer]
                    .checked_sub(1)
                    .ok_or("packet receipt ACK without an outstanding M0 packet")?;
            }
            Event::DurableAck(message) => {
                let state = &mut self.states[message];
                if !state.retained || state.result.ack_tick.is_some() {
                    return Err(format!(
                        "duplicate or ownerless durable ACK for message {message}"
                    ));
                }
                if state
                    .result
                    .modeled_commit_tick
                    .is_none_or(|commit| commit > tick)
                {
                    return Err(format!(
                        "durable ACK precedes modeled commit for message {message}"
                    ));
                }
                state.result.ack_tick = Some(tick);
                state.retained = false;
                let spec = &self.trace.messages[message];
                self.sender_retained[spec.producer] -= spec.bytes;
                self.total_sender_retained -= spec.bytes;
                self.summary.ack_count += 1;
            }
        }
        Ok(())
    }

    fn grant_available(&mut self, tick: u64) -> Result<(), String> {
        while self.grants_outstanding < self.config.bdp_packets {
            let mut selected = None;
            for offset in 0..self.config.producers {
                let producer = (self.next_grant_producer + offset) % self.config.producers;
                let messages = &self.producer_messages[producer];
                while self.grant_cursor[producer] < messages.len()
                    && self.states[messages[self.grant_cursor[producer]]].granted_packets
                        == self.config.packets_for(self.variant)
                            - usize::from(self.variant == Variant::M2)
                {
                    self.grant_cursor[producer] += 1;
                }
                if let Some(&message) = messages.get(self.grant_cursor[producer]) {
                    if self.states[message].announced {
                        selected = Some((producer, message));
                        break;
                    }
                }
            }
            let Some((producer, message)) = selected else {
                break;
            };
            self.states[message].granted_packets += 1;
            self.grants_outstanding += 1;
            check_credit_budget(self.grants_outstanding, self.config.bdp_packets)?;
            self.summary.max_granted_outstanding_packets = self
                .summary
                .max_granted_outstanding_packets
                .max(self.grants_outstanding);
            self.control();
            self.schedule(
                tick + self.config.control_delay_ticks,
                Event::Grant(message),
            );
            self.next_grant_producer = (producer + 1) % self.config.producers;
        }
        Ok(())
    }

    fn send_packets(&mut self, tick: u64) -> Result<(), String> {
        for producer in 0..self.config.producers {
            let messages = &self.producer_messages[producer];
            while self.send_cursor[producer] < messages.len()
                && self.states[messages[self.send_cursor[producer]]].sent_packets
                    == self.config.packets_for(self.variant)
            {
                self.send_cursor[producer] += 1;
            }
            let Some(&message) = messages.get(self.send_cursor[producer]) else {
                continue;
            };
            if self.trace.messages[message].release_tick > tick {
                continue;
            }
            let packet_index = self.states[message].sent_packets;
            let scheduled =
                self.variant == Variant::M1 || (self.variant == Variant::M2 && packet_index > 0);
            if self.variant == Variant::M0 {
                if self.sender_window_outstanding[producer] >= self.config.bdp_packets {
                    continue;
                }
                self.sender_window_outstanding[producer] += 1;
                self.summary.max_sender_window_outstanding_packets = self
                    .summary
                    .max_sender_window_outstanding_packets
                    .max(self.sender_window_outstanding[producer]);
            } else if scheduled {
                if self.states[message].available_credits == 0 {
                    continue;
                }
                self.states[message].available_credits -= 1;
            }
            let payload_bytes = self.config.packet_payload_bytes(self.variant, packet_index);
            let wire_bytes = payload_bytes
                + if self.variant == Variant::M2 && packet_index == 0 {
                    EMBEDDED_ANNOUNCEMENT_BYTES
                } else {
                    0
                };
            self.states[message].sent_packets += 1;
            self.states[message]
                .result
                .first_send_tick
                .get_or_insert(tick);
            self.sender_queued[producer] = self.sender_queued[producer]
                .checked_sub(payload_bytes)
                .ok_or("sender queue byte accounting underflow")?;
            self.total_sender_queued -= payload_bytes;
            self.summary.data_packets_sent += 1;
            self.summary.data_wire_bytes_sent += wire_bytes as u64;
            if scheduled {
                self.summary.scheduled_packets_sent += 1;
            } else {
                self.summary.unscheduled_packets_sent += 1;
            }
            if self.variant == Variant::M2 && packet_index == 0 {
                self.summary.embedded_metadata_bytes += EMBEDDED_ANNOUNCEMENT_BYTES as u64;
            }
            self.schedule(
                tick + self.config.data_delay_ticks,
                Event::DataToSwitch(Packet {
                    message,
                    payload_bytes,
                    wire_bytes,
                    scheduled,
                }),
            );
        }
        Ok(())
    }

    fn sample_queues(&mut self) {
        self.summary.sender_peak_bytes =
            self.summary.sender_peak_bytes.max(self.total_sender_queued);
        self.summary.sender_retained_peak_bytes = self
            .summary
            .sender_retained_peak_bytes
            .max(self.total_sender_retained);
        self.summary.max_sender_retained_per_producer_bytes = self
            .summary
            .max_sender_retained_per_producer_bytes
            .max(*self.sender_retained.iter().max().unwrap_or(&0));
        self.summary.max_sender_queue_per_producer_bytes = self
            .summary
            .max_sender_queue_per_producer_bytes
            .max(*self.sender_queued.iter().max().unwrap_or(&0));
        self.summary.switch_peak_bytes =
            self.summary.switch_peak_bytes.max(self.switch_queued_bytes);
        self.summary.total_queue_peak_bytes = self
            .summary
            .total_queue_peak_bytes
            .max(self.total_sender_queued + self.switch_queued_bytes);
        self.summary.sender_byte_ticks += self.total_sender_queued as u128;
        self.summary.switch_byte_ticks += self.switch_queued_bytes as u128;
        self.summary.total_queue_byte_ticks +=
            (self.total_sender_queued + self.switch_queued_bytes) as u128;
        for &burst in &self.active_bursts {
            self.bursts[burst].peak_hotspot_bytes = self.bursts[burst]
                .peak_hotspot_bytes
                .max(self.switch_queued_bytes);
        }
    }

    fn transmit_one(&mut self, tick: u64) {
        if let Some(packet) = self.switch.pop_front() {
            self.switch_queued_bytes -= packet.wire_bytes;
            // One egress slot is consumed over [tick, tick + 1); delivery is at tick + 1.
            self.schedule(tick + 1, Event::ReceiverDelivery(packet));
        }
    }

    fn run(mut self, seed: u64) -> Result<Run, String> {
        let mut next_release = 0;
        for tick in 0..=self.config.drain_deadline_ticks {
            if next_release < self.config.bursts && self.trace.releases[next_release] == tick {
                self.release_burst(next_release, tick)?;
                next_release += 1;
            }
            let slot = tick as usize % self.events.len();
            let due = std::mem::take(&mut self.events[slot]);
            for event in due {
                self.receive_event(tick, event)?;
            }
            if self.variant != Variant::M0 {
                self.grant_available(tick)?;
            }
            // All scheduled data arrivals are already in the switch queue. Sample
            // before sender wire-send and switch egress. Sender bytes are unsent
            // payload only; retained ownership is sampled separately.
            self.sample_queues();
            self.active_bursts
                .retain(|&burst| self.burst_remaining[burst] > 0);
            self.send_packets(tick)?;
            self.transmit_one(tick);
        }
        if next_release != self.config.bursts {
            return Err("trace not fully released by drain deadline".into());
        }
        self.summary.completed_deadline_messages = self
            .states
            .iter()
            .filter(|state| state.result.complete_tick.is_some())
            .count();
        self.summary.unacked_at_deadline = self
            .states
            .iter()
            .filter(|state| state.result.ack_tick.is_none())
            .count();
        if self.summary.completed_deadline_messages != self.trace.messages.len()
            || self.summary.unacked_at_deadline != 0
        {
            return Err(format!(
                "incomplete run: {} of {} messages completed, {} durable ACKs pending at deadline",
                self.summary.completed_deadline_messages,
                self.trace.messages.len(),
                self.summary.unacked_at_deadline
            ));
        }
        if self.summary.data_packets_sent != self.summary.data_packets_delivered
            || self.grants_outstanding != 0
            || self.total_sender_queued != 0
            || self.total_sender_retained != 0
            || !self.switch.is_empty()
        {
            return Err("packet/credit/ownership conservation failed at deadline".into());
        }
        let mut peaks: Vec<_> = self
            .bursts
            .iter()
            .map(|burst| burst.peak_hotspot_bytes)
            .collect();
        peaks.sort_unstable();
        let rank = (99 * peaks.len()).div_ceil(100);
        self.summary.p99_burst_peak_hotspot_bytes = peaks[rank - 1];
        Ok(Run {
            seed,
            variant: self.variant,
            config: self.config.clone(),
            trace: self.trace.clone(),
            messages: self.states.into_iter().map(|state| state.result).collect(),
            bursts: self.bursts,
            summary: self.summary,
        })
    }
}

pub fn simulate(
    config: &Config,
    trace: &Trace,
    seed: u64,
    variant: Variant,
) -> Result<Run, String> {
    config.validate()?;
    if variant == Variant::M2
        && (config.unscheduled_prefix_packets != 1
            || config.packet_size <= EMBEDDED_ANNOUNCEMENT_BYTES)
    {
        return Err("M2 needs one unscheduled prefix packet with payload room".into());
    }
    if trace.releases.len() != config.bursts
        || trace.messages.len() != config.bursts * config.producers
    {
        return Err("trace dimensions do not match config".into());
    }
    for (burst, &release) in trace.releases.iter().enumerate() {
        if burst > 0 && release <= trace.releases[burst - 1] {
            return Err("trace releases must be strictly increasing".into());
        }
        for producer in 0..config.producers {
            let spec = &trace.messages[burst * config.producers + producer];
            if spec.producer != producer
                || spec.sequence != burst + 1
                || spec.burst != burst
                || spec.bytes != config.message_bytes
                || spec.release_tick != release
            {
                return Err(format!(
                    "invalid or duplicate trace identity at burst {burst}, producer {producer}"
                ));
            }
        }
    }
    Simulator::new(config, trace, variant).run(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny(producers: usize, bursts: usize, bytes: usize) -> Config {
        Config {
            producers,
            bursts,
            message_bytes: bytes,
            packet_size: 1_500,
            data_delay_ticks: 4,
            control_delay_ticks: 4,
            bdp_packets: 9,
            unscheduled_prefix_packets: 0,
            sender_queue_cap_bytes: bytes * 2,
            switch_queue_cap_bytes: 512 * 1_500,
            injection_interval_ticks: 100_000,
            drain_deadline_ticks: 110_000,
            nominal_burst_spacing_ticks: 1_760,
            jitter_magnitude_ticks: 0,
        }
    }

    #[test]
    fn one_message_serialization_and_propagation() {
        let config = tiny(1, 1, 3_001);
        let trace = Trace::generate(&config, 0).unwrap();
        let m0 = simulate(&config, &trace, 0, Variant::M0).unwrap();
        let m1 = simulate(&config, &trace, 0, Variant::M1).unwrap();
        assert_eq!(m0.messages[0].first_send_tick, Some(0));
        assert_eq!(m0.messages[0].complete_tick, Some(7)); // send 0..2, +4 arrival, +1 egress
        assert_eq!(m0.messages[0].modeled_commit_tick, Some(7));
        assert_eq!(m0.messages[0].ack_tick, Some(11));
        assert_eq!(m1.messages[0].first_send_tick, Some(8)); // announcement +4, grant +4
        assert_eq!(m1.messages[0].complete_tick, Some(15));
        assert_eq!(m1.messages[0].ack_tick, Some(19));
        assert_eq!(m0.summary.data_packets_delivered, 3);
        assert_eq!(m1.summary.data_packets_delivered, 3);
    }

    #[test]
    fn credit_budget_and_injected_overgrant() {
        let config = tiny(32, 2, 65_536);
        let trace = Trace::generate(&config, 7).unwrap();
        let run = simulate(&config, &trace, 7, Variant::M1).unwrap();
        assert_eq!(run.summary.max_granted_outstanding_packets, 9);
        assert!(run.summary.switch_peak_bytes <= 9 * config.packet_size);
        // Inject the characteristic broken transition: issue one more grant
        // without a delivery after the budget is full. The same invariant used
        // by the simulator must reject that state.
        let faulty_outstanding = run.summary.max_granted_outstanding_packets + 1;
        assert!(check_credit_budget(faulty_outstanding, config.bdp_packets).is_err());
    }

    #[test]
    fn trace_identities_and_jitter_are_stable() {
        let config = tiny(3, 4, 1_500);
        let trace = Trace::generate(&config, 0).unwrap();
        let ids: std::collections::HashSet<_> = trace
            .messages
            .iter()
            .map(|message| (message.producer, message.sequence))
            .collect();
        assert_eq!(ids.len(), 12);
        assert_eq!(trace.releases, vec![0, 1_760, 3_520, 5_280]);
        assert_eq!(splitmix64_next(&mut 0), 0xe220_a839_7b1d_cdaf);
    }

    #[test]
    fn synchronized_m0_window_creates_n_times_u_arrivals() {
        let producers = 5;
        let unscheduled_packets_per_sender = 2;
        let packet_size = 1_500;
        let config = tiny(producers, 1, unscheduled_packets_per_sender * packet_size);
        let trace = Trace::generate(&config, 0).unwrap();
        let run = simulate(&config, &trace, 0, Variant::M0).unwrap();
        // N*U packets enter the switch in two synchronized waves. One packet
        // leaves after the first wave, so the second wave's pre-egress peak is
        // N*U-1 packet slots in this exact tiny schedule.
        assert_eq!(
            run.summary.data_packets_sent as usize,
            producers * unscheduled_packets_per_sender
        );
        assert_eq!(
            run.summary.switch_peak_bytes,
            (producers * unscheduled_packets_per_sender - 1) * packet_size
        );
        assert_eq!(
            run.summary.data_packets_delivered,
            run.summary.data_packets_sent
        );
    }

    #[test]
    fn retained_copy_cap_is_checked_separately_from_unsent_queue() {
        let mut config = tiny(1, 3, 1_500);
        config.nominal_burst_spacing_ticks = 1;
        let trace = Trace::generate(&config, 0).unwrap();
        let error = simulate(&config, &trace, 0, Variant::M0).unwrap_err();
        assert!(error.contains("retained-copy cap overflow"), "{error}");
    }

    #[test]
    fn incomplete_run_is_an_error() {
        let mut config = tiny(1, 1, 1_500);
        config.drain_deadline_ticks = 4;
        config.injection_interval_ticks = 4;
        let trace = Trace::generate(&config, 0).unwrap();
        let error = simulate(&config, &trace, 0, Variant::M0).unwrap_err();
        assert!(error.contains("incomplete run"), "{error}");
    }
}
