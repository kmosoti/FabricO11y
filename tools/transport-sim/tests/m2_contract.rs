//! Independent black-box checks for the registered M2 packet-slot contract.
//! See docs/experiments/ablation/unscheduled-prefix-m2-run-01.md.

use transport_sim::{Config, Trace, Variant, simulate};

fn one_burst(producers: usize, message_bytes: usize) -> Config {
    let mut config = Config::m2_small();
    config.producers = producers;
    config.bursts = 1;
    config.message_bytes = message_bytes;
    config.jitter_magnitude_ticks = 0;
    config.injection_interval_ticks = 1_000;
    config.drain_deadline_ticks = 1_000;
    config.sender_queue_cap_bytes = 100_000;
    config.switch_queue_cap_bytes = 100_000;
    config
}

#[test]
fn tiny_message_removes_request_and_credit_startup_without_early_ack() {
    let config = one_burst(1, 148);
    let trace = Trace::generate(&config, 0).expect("valid one-message trace");

    let m1 = simulate(&config, &trace, 0, Variant::M1).expect("M1 control run");
    let m2 = simulate(&config, &trace, 0, Variant::M2).expect("M2 run");
    assert_eq!(m1.messages.len(), 1);
    assert_eq!(m2.messages.len(), 1);

    let control = &m1.messages[0];
    assert_eq!(control.first_send_tick, Some(8));
    assert_eq!(control.complete_tick, Some(13));
    assert_eq!(control.modeled_commit_tick, Some(13));
    assert_eq!(control.ack_tick, Some(17));

    let prefix = &m2.messages[0];
    assert_eq!(prefix.first_send_tick, Some(0));
    assert_eq!(prefix.complete_tick, Some(5));
    assert_eq!(prefix.modeled_commit_tick, Some(5));
    assert_eq!(prefix.ack_tick, Some(9));
    assert_eq!(m2.summary.unscheduled_packets_sent, 1);
    assert_eq!(m2.summary.scheduled_packets_sent, 0);
    assert_eq!(m2.summary.embedded_metadata_bytes, 32);
    assert_eq!(m2.summary.data_wire_bytes_sent, 180);
    assert_eq!(m2.summary.sender_retained_peak_bytes, 148);
    assert_eq!(m2.summary.max_sender_retained_per_producer_bytes, 148);
    assert_eq!(m2.summary.ack_count, 1);
    assert_eq!(m2.summary.unacked_at_deadline, 0);
}

#[test]
fn first_packet_reserves_thirty_two_wire_bytes_for_announcement() {
    let config = one_burst(1, 1_500);
    let trace = Trace::generate(&config, 0).expect("valid packet-boundary trace");
    let run = simulate(&config, &trace, 0, Variant::M2).expect("M2 boundary run");

    // 1,468 payload + 32 announcement fills the first 1,500-byte wire slot.
    // The remaining 32 payload bytes need a credited second DATA slot.
    assert_eq!(run.summary.data_packets_sent, 2);
    assert_eq!(run.summary.data_packets_delivered, 2);
    assert_eq!(run.summary.unscheduled_packets_sent, 1);
    assert_eq!(run.summary.scheduled_packets_sent, 1);
    assert_eq!(run.summary.embedded_metadata_bytes, 32);
    assert_eq!(run.summary.data_wire_bytes_sent, 1_532);
    assert!(run.messages[0].complete_tick.unwrap() > 5);
    assert_eq!(run.summary.unacked_at_deadline, 0);
}

#[test]
fn thirty_two_unscheduled_packets_make_a_48_000_byte_first_wave() {
    let mut config = one_burst(32, 1_500);
    config.switch_queue_cap_bytes = 48_000;
    let trace = Trace::generate(&config, 0).expect("valid synchronized incast trace");
    let run = simulate(&config, &trace, 0, Variant::M2).expect("exact-cap incast");

    assert_eq!(run.summary.switch_peak_bytes, 48_000);
    assert_eq!(run.summary.unscheduled_packets_sent, 32);
    assert_eq!(run.summary.scheduled_packets_sent, 32);
    assert_eq!(run.summary.embedded_metadata_bytes, 32 * 32);
    assert_eq!(run.summary.data_wire_bytes_sent, 32 * 1_532);
    assert_eq!(run.summary.max_granted_outstanding_packets, 9);
    assert_eq!(run.summary.cap_overflow_count, 0);
    assert_eq!(run.summary.unacked_at_deadline, 0);

    config.switch_queue_cap_bytes = 47_999;
    let under_cap_trace = Trace::generate(&config, 0).expect("same trace under smaller cap");
    let error = simulate(&config, &under_cap_trace, 0, Variant::M2)
        .expect_err("a 47,999-byte cap must reject the first arrival wave");
    assert!(error.contains("switch queue cap overflow"), "{error}");
}

#[test]
fn one_uncredited_packet_per_message_and_all_remainders_need_grants() {
    let config = one_burst(2, 3_001);
    let trace = Trace::generate(&config, 0).expect("valid multipacket trace");
    let run = simulate(&config, &trace, 0, Variant::M2).expect("M2 multipacket run");

    // Per message: 1,468 + 1,500 + 33 payload bytes in three DATA slots.
    assert_eq!(run.summary.unscheduled_packets_sent, 2);
    assert_eq!(run.summary.scheduled_packets_sent, 4);
    assert_eq!(run.summary.data_packets_sent, 6);
    assert_eq!(run.summary.data_packets_delivered, 6);
    assert_eq!(run.summary.embedded_metadata_bytes, 64);
    assert_eq!(run.summary.data_wire_bytes_sent, 2 * (3_001 + 32));
    assert!(run.summary.max_granted_outstanding_packets > 0);
    assert!(run.summary.max_granted_outstanding_packets <= 9);
    assert_eq!(run.summary.ack_count, 2);
    assert_eq!(run.summary.unacked_at_deadline, 0);
}

#[test]
fn a_sent_prefix_does_not_release_the_retained_source() {
    let mut config = one_burst(1, 148);
    config.bursts = 2;
    config.sender_queue_cap_bytes = 148;
    config.nominal_burst_spacing_ticks = 1;
    let overlapping = Trace::generate(&config, 0).expect("valid overlapping trace");
    assert!(
        simulate(&config, &overlapping, 0, Variant::M2).is_err(),
        "second source copy must exceed the retained-copy cap before ACK"
    );

    config.nominal_burst_spacing_ticks = 10;
    let after_ack = Trace::generate(&config, 0).expect("valid separated trace");
    let run = simulate(&config, &after_ack, 0, Variant::M2)
        .expect("the first ACK frees room for the second source copy");
    assert_eq!(run.summary.ack_count, 2);
    assert_eq!(run.summary.max_sender_retained_per_producer_bytes, 148);
    assert_eq!(run.summary.unacked_at_deadline, 0);
}
