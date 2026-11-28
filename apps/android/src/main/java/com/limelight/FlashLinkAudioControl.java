// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import java.io.IOException;
import java.io.InputStream;
import javax.net.ssl.SSLHandshakeException;

final class FlashLinkAudioControl {
    /** State of one lease. Its owner serializes lifecycle events and report snapshots. */
    static final class Session {
        final long id;
        private long sequence;
        private String phase;
        private long muteConfirmedAt = -1;

        Session(long id, boolean prepare) {
            this.id = id;
            phase = prepare ? "prepare" : "live";
        }

        boolean preparing() { return phase.equals("prepare"); }
        boolean terminal() { return !preparing() && !phase.equals("live"); }
        String phase() { return phase; }

        void confirmed(boolean muted, long now) {
            if (preparing()) muteConfirmedAt = muted ? now : -1;
        }

        boolean canStart(long now) {
            return preparing() && muteConfirmedAt >= 0 && now - muteConfirmedAt <= 3000;
        }

        void connected() {
            if (preparing() && muteConfirmedAt >= 0) phase = "live";
        }

        boolean finish(String requested) {
            if (terminal()) return false;
            phase = preparing() ? "cancel" : requested;
            return true;
        }

        String report(String sample) {
            // A live privacy heartbeat remains useful when the first audio sample is unavailable.
            if (sample == null) return phase.equals("live") ? "FLASHLINK_STREAM/1" : null;
            return "FLASHLINK_STREAM/2 " + id + " " + (++sequence) + " " + phase + " " + sample;
        }
    }

    interface Request {
        boolean send(String message) throws IOException;
    }

    static boolean readReply(InputStream input) throws IOException {
        int first = input.read(), second = input.read(), newline = input.read();
        if (newline == '\n') {
            if (first == 'O' && second == 'K') return true;
            if (first == 'N' && second == 'O') return false;
        }
        throw new IOException("Incomplete or invalid audio control reply");
    }

    static boolean exchange(Request request, String message, int attempts) throws IOException {
        for (int i = 1; ; i++) {
            try {
                return request.send(message);
            } catch (SSLHandshakeException error) {
                throw error;
            } catch (IOException error) {
                if (i >= attempts) throw error;
            }
        }
    }

    static int capability(Request request) throws IOException {
        if (exchange(request, "FLASHLINK_AUDIO_CAPS/2", 2)) return 2;
        return exchange(request, "FLASHLINK_AUDIO_CAPS/1", 2) ? 1 : 0;
    }
}
