// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

public final class FlashLinkAudioControlTest {
    private static boolean reply(String value) throws IOException {
        return FlashLinkAudioControl.readReply(new ByteArrayInputStream(value.getBytes(StandardCharsets.US_ASCII)));
    }

    private static void preflightLifecycle() throws IOException {
        String sample = "controller sync 40 0";
        FlashLinkAudioControl.Session lease = new FlashLinkAudioControl.Session(42, true);
        assert !lease.canStart(1000);
        assert lease.report(null) == null;
        lease.connected(); // No confirmation, no promotion.
        assert lease.preparing();
        String first = lease.report(sample);
        assert first.equals("FLASHLINK_STREAM/2 42 1 prepare " + sample);
        int[] calls = {0};
        boolean accepted = FlashLinkAudioControl.exchange(message -> {
            assert first.equals(message); // Lost OK retries the same session, sequence and phase.
            if (++calls[0] == 1) throw new IOException("lost reply");
            return true;
        }, first, 2);
        lease.confirmed(accepted, 1000);
        assert lease.canStart(1001);
        assert !lease.canStart(4001);
        assert lease.report(sample).equals("FLASHLINK_STREAM/2 42 2 prepare " + sample);
        lease.confirmed(true, 4100);
        assert lease.canStart(4100);
        lease.connected();
        assert !lease.canStart(4101);
        assert lease.report(sample).equals("FLASHLINK_STREAM/2 42 3 live " + sample);
        assert lease.report(null).equals("FLASHLINK_STREAM/1");
        assert lease.finish("end");
        assert lease.report(null) == null;
        assert !lease.finish("cancel");
        assert lease.report(sample).equals("FLASHLINK_STREAM/2 42 4 end " + sample);

        for (String end : new String[]{"end", "drop", "handoff"}) {
            lease = new FlashLinkAudioControl.Session(43, true);
            lease.confirmed(true, 10);
            lease.confirmed(false, 11);
            assert !lease.canStart(11);
            assert lease.finish(end);
            lease.confirmed(true, 12); // Previous in-flight reply after cancellation.
            lease.connected();
            assert !lease.canStart(12);
            assert lease.report(sample).equals("FLASHLINK_STREAM/2 43 1 cancel " + sample);
        }
        lease = new FlashLinkAudioControl.Session(44, false);
        assert lease.report(sample).equals("FLASHLINK_STREAM/2 44 1 live " + sample);
        assert lease.finish("handoff");
        assert lease.report(sample).equals("FLASHLINK_STREAM/2 44 2 handoff " + sample);
    }

    public static void main(String[] args) throws IOException {
        preflightLifecycle();
        assert reply("OK\n");
        assert !reply("NO\n");
        for (String truncated : new String[]{"", "O", "NO", "XX\n"}) {
            try {
                reply(truncated);
                throw new AssertionError(truncated);
            } catch (IOException expected) { }
        }
        int[] attempts = {0};
        assert FlashLinkAudioControl.capability(message -> {
            assert message.equals("FLASHLINK_AUDIO_CAPS/2");
            if (++attempts[0] == 1) throw new IOException("temporary loss");
            return true;
        }) == 2;
        assert attempts[0] == 2;
        assert FlashLinkAudioControl.capability(message -> message.endsWith("/1")) == 1;
        assert FlashLinkAudioControl.capability(message -> false) == 0;
        attempts[0] = 0;
        try {
            FlashLinkAudioControl.capability(message -> {
                attempts[0]++;
                throw new IOException("unreachable");
            });
            throw new AssertionError("transport failure treated as legacy support");
        } catch (IOException expected) { assert attempts[0] == 2; }
    }
}
