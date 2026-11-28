// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

/** Owns one player's requests. All calls and callbacks are serialized on the UI thread. */
final class FlashLinkLiveBitrate {
    enum State { NEW, PROBING, READY, SETTING, VERIFYING, UNKNOWN, UNAVAILABLE, CLOSED }
    enum Result { CURRENT, APPLIED, REJECTED, UNCONFIRMED }
    interface Listener {
        void send(int id, int kbps);
        void result(Result result, int effectiveKbps);
    }

    private final Listener listener;
    private State state = State.NEW;
    private int sequence;
    private int requested;

    FlashLinkLiveBitrate(Listener listener) { this.listener = listener; }
    State state() { return state; }

    void probe() {
        if (state == State.NEW) issue(State.PROBING, 0);
        else if (state == State.UNKNOWN) issue(State.VERIFYING, 0);
    }

    boolean set(int kbps) {
        if (state != State.READY || !valid(kbps)) return false;
        requested = kbps;
        issue(State.SETTING, kbps);
        return true;
    }

    private void issue(State operation, int kbps) {
        state = operation;
        if (++sequence == 0) ++sequence;
        listener.send(sequence, kbps);
    }

    void response(int id, int status, int kbps) {
        if (!waiting(id)) return;
        if (status < 0 || status > 3 || (status == 0 && !valid(kbps))) return;

        if (state == State.PROBING) {
            state = status == 0 ? State.READY : State.UNAVAILABLE;
            listener.result(Result.CURRENT, status == 0 ? kbps : 0);
            return;
        }
        if (state == State.VERIFYING && status != 0) {
            state = status == 1 ? State.UNAVAILABLE : State.UNKNOWN;
            listener.result(Result.UNCONFIRMED, 0);
            return;
        }

        boolean applied = status == 0 && (state == State.SETTING || kbps == requested);
        state = status == 1 ? State.UNAVAILABLE : State.READY;
        requested = 0;
        listener.result(applied ? Result.APPLIED : Result.REJECTED, valid(kbps) ? kbps : 0);
    }

    void timeout(int id) {
        if (!waiting(id)) return;
        if (state == State.SETTING) {
            // A SET may have reached the encoder even when its reply hasn't reached us.
            issue(State.VERIFYING, 0);
        } else if (state == State.PROBING) {
            state = State.UNAVAILABLE;
            listener.result(Result.CURRENT, 0);
        } else {
            state = State.UNKNOWN;
            listener.result(Result.UNCONFIRMED, 0);
        }
    }

    void close() { state = State.CLOSED; }
    private boolean waiting(int id) {
        return sequence == id && (state == State.PROBING || state == State.SETTING || state == State.VERIFYING);
    }

    private static boolean valid(int kbps) { return kbps >= 500 && kbps <= 150000; }
}
