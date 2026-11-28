// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

/** Waits for both the encoder reply and a decoded frame, in either arrival order. */
final class FlashLinkResolution {
    interface Listener {
        boolean request(int size);
        void finished(int size, boolean saved);
        void unconfirmed();
    }
    private final Listener listener;
    private int previous, target, presented, generation;
    private int confirmed, fallbackSize;
    private boolean acknowledged, restoring, restorePreference;

    FlashLinkResolution(Listener listener) { this.listener = listener; }
    static int pack(int width, int height) { return (width << 16) | height; }
    static boolean valid(int size) {
        int w = size >>> 16, h = size & 0xffff;
        return w >= 64 && h >= 64 && w <= 4096 && h <= 4096 && w * h <= 4096 * 2160 && ((w | h) & 1) == 0;
    }

    boolean pending() { return target != 0; }
    int generation() { return generation; }

    boolean begin(int current, int next) {
        if (pending() || !valid(next) || next == current) return false;
        previous = presented = current;
        restoring = acknowledged = restorePreference = false;
        return request(next);
    }

    private boolean request(int next) {
        ++generation;
        target = next;
        if (listener.request(next)) return true;
        target = 0;
        return false;
    }

    void presented(int value) {
        presented = value;
        finishIfReady();
    }

    void response(FlashLinkLiveSetting.Result result, int value) {
        if (!pending()) return;
        if (result == FlashLinkLiveSetting.Result.APPLIED && value == target) {
            acknowledged = true;
            finishIfReady();
        } else if (!restoring && result == FlashLinkLiveSetting.Result.REJECTED &&
                presented == previous && (value == previous || value == 0)) {
            target = 0;
            listener.finished(previous, false);
        }
        // Uncertain outcomes use the same bounded recovery as a stalled decoder.
    }

    private void finishIfReady() {
        if (pending() && acknowledged && presented == target) {
            target = 0;
            if (!restoring) {
                confirmed = presented;
                fallbackSize = previous;
            }
            listener.finished(presented, !restoring || restorePreference);
        }
    }

    void timeout(int token) {
        if (!pending() || token != generation) return;
        if (!restoring) {
            restoring = true;
            acknowledged = false;
            // The decoder may still be showing its last good frame from before the request.
            presented = 0;
            if (request(previous)) return;
        }
        target = 0;
        confirmed = 0;
        listener.unconfirmed();
    }

    boolean inputTooSmall(int size) {
        if (!pending() && confirmed != 0 && size == confirmed && presented == size) {
            confirmed = 0;
            previous = fallbackSize;
            restoring = restorePreference = true;
            acknowledged = false;
            if (!request(previous)) listener.unconfirmed();
            return true;
        }
        return false;
    }

    void close() {
        target = 0;
        confirmed = 0;
    }
}
