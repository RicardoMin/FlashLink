// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

/** Owns one player's requests. All calls and callbacks are serialized on the UI thread. */
final class FlashLinkLiveSetting {
    enum Kind {
        BITRATE(500, 150000), FRAME_RATE(1, 240), RESOLUTION(0, 0);

        private final int min, max;
        Kind(int min, int max) { this.min = min; this.max = max; }
        boolean valid(int value) {
            return this == RESOLUTION ? FlashLinkResolution.valid(value) : value >= min && value <= max;
        }
    }

    enum State { NEW, PROBING, READY, SETTING, VERIFYING, UNKNOWN, UNAVAILABLE, CLOSED }
    enum Result { CURRENT, APPLIED, REJECTED, UNCONFIRMED }
    interface Listener {
        void send(int id, int value);
        void result(Result result, int effectiveValue);
    }

    private final Kind kind;
    private final Listener listener;
    private State state = State.NEW;
    private int sequence;
    private int requested;

    FlashLinkLiveSetting(Kind kind, Listener listener) {
        this.kind = kind;
        this.listener = listener;
    }
    State state() { return state; }
    boolean waiting() { return state == State.PROBING || state == State.SETTING || state == State.VERIFYING; }
    boolean probing() { return state == State.PROBING || state == State.UNKNOWN; }

    void probe() {
        if (state == State.NEW) issue(State.PROBING, 0);
        else if (state == State.UNKNOWN) issue(State.VERIFYING, 0);
    }

    boolean set(int value) {
        if (state != State.READY || !kind.valid(value)) return false;
        requested = value;
        issue(State.SETTING, value);
        return true;
    }

    private void issue(State operation, int value) {
        state = operation;
        if (++sequence == 0) ++sequence;
        listener.send(sequence, value);
    }

    void response(int id, int status, int value) {
        if (!waiting(id)) return;
        if (status < 0 || status > 3 || (status == 0 && !kind.valid(value))) return;

        if (state == State.PROBING) {
            state = status == 0 ? State.READY : State.UNAVAILABLE;
            listener.result(Result.CURRENT, status == 0 ? value : 0);
            return;
        }
        if (state == State.VERIFYING && status != 0) {
            state = status == 1 ? State.UNAVAILABLE : State.UNKNOWN;
            listener.result(Result.UNCONFIRMED, 0);
            return;
        }

        boolean applied = status == 0 && value == requested;
        state = status == 1 ? State.UNAVAILABLE : State.READY;
        requested = 0;
        listener.result(applied ? Result.APPLIED : Result.REJECTED, kind.valid(value) ? value : 0);
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
        return sequence == id && waiting();
    }

}
