pub(crate) mod cubic;
mod hybrid_slow_start;
mod prr;

use std::fmt::Debug;
use std::time::Duration;
use std::time::Instant;

use crate::minmax::Minmax;

use super::Acked;

pub(crate) use hybrid_slow_start::HybridSlowStart;
pub(crate) use prr::PrrSender;

const RTT_WINDOW: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub struct Lost {
    pub(super) packet_number: u64,
    pub(super) bytes_lost: usize,
}

const INITIAL_RTT: Duration = Duration::from_millis(333);

const MAX_SEGMENT_SIZE: usize = 1460;

pub struct RttStats {
    pub(super) latest_rtt: Duration,
    pub(super) min_rtt: Minmax<Duration>,
    pub(super) smoothed_rtt: Duration,
    pub(super) rttvar: Duration,
    first_rtt_sample: Option<Instant>,
}

impl Default for RttStats {
    fn default() -> Self {
        RttStats {
            latest_rtt: Duration::ZERO,
            min_rtt: Minmax::new(Duration::ZERO),
            smoothed_rtt: INITIAL_RTT,
            rttvar: INITIAL_RTT / 2,
            first_rtt_sample: None,
        }
    }
}

impl std::fmt::Debug for RttStats {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("RttStats")
            .field("lastest_rtt", &self.latest_rtt)
            .field("srtt", &self.smoothed_rtt)
            .field("minrtt", &*self.min_rtt)
            .field("rttvar", &self.rttvar)
            .finish()
    }
}

impl RttStats {
    pub(crate) fn update_rtt(
        &mut self, latest_rtt: Duration, ack_delay: Duration, now: Instant,
    ) {
        if self.first_rtt_sample.is_none() {
            self.latest_rtt = latest_rtt;
            self.min_rtt.reset(now, latest_rtt);
            self.smoothed_rtt = latest_rtt;
            self.rttvar = latest_rtt / 2;
            self.first_rtt_sample = Some(now);
            return;
        }

        self.latest_rtt = latest_rtt;

        // min_rtt ignores acknowledgment delay.
        self.min_rtt.running_min(RTT_WINDOW, now, latest_rtt);
        // Limit ack_delay by max_ack_delay after handshake
        // confirmation.
        // TODO: if (handshake confirmed):
        //  ack_delay = min(ack_delay, max_ack_delay)

        // Adjust for acknowledgment delay if plausible.
        let mut adjusted_rtt = latest_rtt;
        if latest_rtt >= *self.min_rtt + ack_delay {
            adjusted_rtt = latest_rtt - ack_delay;
        }

        self.rttvar = self.rttvar * 3 / 4 +
            Duration::from_nanos(
                self.smoothed_rtt
                    .as_nanos()
                    .abs_diff(adjusted_rtt.as_nanos()) as u64 /
                    4,
            );
        self.smoothed_rtt = self.smoothed_rtt * 7 / 8 + adjusted_rtt / 8;
    }
}

pub trait CongestionControl: Debug {
    /// Returns the size of the current congestion window in bytes.  Note, this
    /// is not the *available* window.  Some send algorithms may not use a
    /// congestion window and will return 0.
    fn get_congestion_window(&self) -> usize;

    /// Make decision on whether the sender can send right now.  Note that even
    /// when this method returns true, the sending can be delayed due to pacing.
    fn can_send(&self, bytes_in_flight: usize) -> bool;

    /// Inform that we sent |bytes| to the wire, and if the packet is
    /// retransmittable. |bytes_in_flight| is the number of bytes in flight
    /// before the packet was sent. Note: this function must be called for
    /// every packet sent to the wire.
    fn on_packet_sent(
        &mut self, sent_time: Instant, bytes_in_flight: usize,
        packet_number: u64, bytes: usize, is_retransmissible: bool,
    );

    fn on_packet_acked(
        &mut self, acked_packet_number: u64, acked_bytes: usize,
        prior_in_flight: usize, event_time: Instant, min_rtt: Duration,
    );

    /// Indicates an update to the congestion state, caused either by an
    /// incoming ack or loss event timeout.  |rtt_updated| indicates whether a
    /// new latest_rtt sample has been taken, |prior_in_flight| the bytes in
    /// flight prior to the congestion event. |acked_packets| and |lost_packets|
    /// are any packets considered acked or lost as a result of the
    /// congestion event.
    fn on_congestion_event<'a>(
        &mut self, rtt_updated: bool, prior_in_flight: usize,
        event_time: Instant, acked_packets: impl IntoIterator<Item = &'a Acked>,
        lost_packets: impl IntoIterator<Item = &'a Lost>, rtt_stats: &RttStats,
    );

    /// Called when an RTO fires.  Resets the retransmission alarm if there are
    /// remaining unacked packets.
    fn on_retransmission_timeout(&mut self, packets_retransmitted: bool);

    /// Called when connection migrates and cwnd needs to be reset.
    fn on_connection_migration(&mut self);

    fn is_cwnd_limited(&self, bytes_in_flight: usize) -> bool;

    fn is_app_limited(&self, bytes_in_flight: usize) -> bool {
        !self.is_cwnd_limited(bytes_in_flight)
    }

    fn on_app_limited(&self, _bytes_in_flight: usize) {}

    fn update_mss(&mut self, _new_mss: usize) {}
}
