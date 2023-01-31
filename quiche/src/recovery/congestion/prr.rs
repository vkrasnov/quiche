use super::MAX_SEGMENT_SIZE;

#[derive(Default, Debug)]
pub(crate) struct PrrSender {
    bytes_sent_since_loss: usize,
    bytes_delivered_since_loss: usize,
    ack_count_since_loss: usize,
    bytes_in_flight_before_loss: usize,
}

impl PrrSender {
    #[inline]
    pub(crate) fn on_packet_lost(&mut self, prior_in_flight: usize) {
        self.bytes_sent_since_loss = 0;
        self.bytes_in_flight_before_loss = prior_in_flight;
        self.bytes_delivered_since_loss = 0;
        self.ack_count_since_loss = 0;
    }

    #[inline]
    pub(crate) fn on_packet_sent(&mut self, sent_bytes: usize) {
        self.bytes_sent_since_loss += sent_bytes;
    }

    #[inline]
    pub(crate) fn on_packet_acked(&mut self, acked_bytes: usize) {
        self.bytes_delivered_since_loss += acked_bytes;
        self.ack_count_since_loss += 1;
    }

    #[inline]
    pub(crate) fn can_send(
        &self, congestion_window: usize, bytes_in_flight: usize,
        slowstart_threshold: usize,
    ) -> bool {
        // Return QuicTime::Zero in order to ensure limited transmit always works.
        if self.bytes_sent_since_loss == 0 || bytes_in_flight < MAX_SEGMENT_SIZE {
            return true;
        }

        if congestion_window > bytes_in_flight {
            // During PRR-SSRB, limit outgoing packets to 1 extra MSS per ack,
            // instead of sending the entire available window. This
            // prevents burst retransmits when more packets are lost
            // than the CWND reduction.   limit = MAX(prr_delivered -
            // prr_out, DeliveredData) + MSS
            if self.bytes_delivered_since_loss +
                self.ack_count_since_loss * MAX_SEGMENT_SIZE <=
                self.bytes_sent_since_loss
            {
                return false;
            }
            return true;
        }
        // Implement Proportional Rate Reduction (RFC6937).
        // Checks a simplified version of the PRR formula that doesn't use
        // division: AvailableSendWindow =
        //   CEIL(prr_delivered * ssthresh / BytesInFlightAtLoss) - prr_sent
        self.bytes_delivered_since_loss * slowstart_threshold >
            self.bytes_sent_since_loss * self.bytes_in_flight_before_loss
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_loss_results_in_send_on_every_other_ack() {
        let mut prr = PrrSender::default();
        let num_packets_in_flight = 50;
        let mut bytes_in_flight = num_packets_in_flight * MAX_SEGMENT_SIZE;
        let ssthresh_after_loss = num_packets_in_flight / 2;
        let congestion_window = ssthresh_after_loss * MAX_SEGMENT_SIZE;

        prr.on_packet_lost(bytes_in_flight);
        // Ack a packet. PRR allows one packet to leave immediately.
        prr.on_packet_acked(MAX_SEGMENT_SIZE);
        bytes_in_flight -= MAX_SEGMENT_SIZE;

        assert!(prr.can_send(
            congestion_window,
            bytes_in_flight,
            ssthresh_after_loss * MAX_SEGMENT_SIZE
        ));

        // Send retransmission.
        prr.on_packet_sent(MAX_SEGMENT_SIZE);
        // PRR shouldn't allow sending any more packets.
        assert!(!prr.can_send(
            congestion_window,
            bytes_in_flight,
            ssthresh_after_loss * MAX_SEGMENT_SIZE
        ));

        // One packet is lost, and one ack was consumed above. PRR now paces
        // transmissions through the remaining 48 acks. PRR will alternatively
        // disallow and allow a packet to be sent in response to an ack.

        for _ in 0..ssthresh_after_loss - 1 {
            // Ack a packet. PRR shouldn't allow sending a packet in response.
            prr.on_packet_acked(MAX_SEGMENT_SIZE);
            bytes_in_flight -= MAX_SEGMENT_SIZE;
            assert!(!prr.can_send(
                congestion_window,
                bytes_in_flight,
                ssthresh_after_loss * MAX_SEGMENT_SIZE
            ));
            // Ack another packet. PRR should now allow sending a packet in
            // response.
            prr.on_packet_acked(MAX_SEGMENT_SIZE);
            bytes_in_flight -= MAX_SEGMENT_SIZE;
            assert!(prr.can_send(
                congestion_window,
                bytes_in_flight,
                ssthresh_after_loss * MAX_SEGMENT_SIZE
            ));
            // Send a packet in response.
            prr.on_packet_sent(MAX_SEGMENT_SIZE);
            bytes_in_flight += MAX_SEGMENT_SIZE;
        }

        // Since bytes_in_flight is now equal to congestion_window, PRR now
        // maintains packet conservation, allowing one packet to be sent
        // in response to an ack.
        assert_eq!(congestion_window, bytes_in_flight);
        for _ in 0..10 {
            // Ack a packet.
            prr.on_packet_acked(MAX_SEGMENT_SIZE);
            bytes_in_flight -= MAX_SEGMENT_SIZE;
            assert!(prr.can_send(
                congestion_window,
                bytes_in_flight,
                ssthresh_after_loss * MAX_SEGMENT_SIZE
            ));
            // Send a packet in response, since PRR allows it.
            prr.on_packet_sent(MAX_SEGMENT_SIZE);
            bytes_in_flight += MAX_SEGMENT_SIZE;

            // Since bytes_in_flight is equal to the congestion_window,
            // PRR disallows sending.
            assert_eq!(congestion_window, bytes_in_flight);
            assert!(!prr.can_send(
                congestion_window,
                bytes_in_flight,
                ssthresh_after_loss * MAX_SEGMENT_SIZE
            ));
        }
    }

    #[test]
    fn burst_loss_results_in_slow_start() {
        let mut prr = PrrSender::default();
        let mut bytes_in_flight = 20 * MAX_SEGMENT_SIZE;
        let num_packets_lost = 13;
        let ssthresh_after_loss = 10;
        let congestion_window = ssthresh_after_loss * MAX_SEGMENT_SIZE;

        // Lose 13 packets.
        bytes_in_flight -= num_packets_lost * MAX_SEGMENT_SIZE;
        prr.on_packet_lost(bytes_in_flight);

        // PRR-SSRB will allow the following 3 acks to send up to 2 packets.
        for _ in 0..3 {
            prr.on_packet_acked(MAX_SEGMENT_SIZE);
            bytes_in_flight -= MAX_SEGMENT_SIZE;
            // PRR-SSRB should allow two packets to be sent.
            for _ in 0..2 {
                assert!(prr.can_send(
                    congestion_window,
                    bytes_in_flight,
                    ssthresh_after_loss * MAX_SEGMENT_SIZE
                ));
                // Send a packet in response.
                prr.on_packet_sent(MAX_SEGMENT_SIZE);
                bytes_in_flight += MAX_SEGMENT_SIZE;
            }
            // PRR should allow no more than 2 packets in response to an ack.
            assert!(!prr.can_send(
                congestion_window,
                bytes_in_flight,
                ssthresh_after_loss * MAX_SEGMENT_SIZE
            ));
        }

        // Out of SSRB mode, PRR allows one send in response to each ack.
        for _ in 0..10 {
            prr.on_packet_acked(MAX_SEGMENT_SIZE);
            bytes_in_flight -= MAX_SEGMENT_SIZE;
            assert!(prr.can_send(
                congestion_window,
                bytes_in_flight,
                ssthresh_after_loss * MAX_SEGMENT_SIZE
            ));
            // Send a packet in response.
            prr.on_packet_sent(MAX_SEGMENT_SIZE);
            bytes_in_flight += MAX_SEGMENT_SIZE;
        }
    }
}
