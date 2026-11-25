// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

/** Uses Android monotonic elapsed time for one interrupted foreground session. */
final class FlashLinkReconnect {
    static final long WINDOW_MS = 15_000;
    private static final long[] DELAYS_MS = {750, 1500, 3000, 4000};
    long deadline;
    int attempts;

    void interrupted(long now, long previousDeadline) {
        deadline = previousDeadline > 0 ? previousDeadline : now + WINDOW_MS;
        attempts = 0;
    }

    long nextDelay(long now) {
        long delay = DELAYS_MS[Math.min(attempts, DELAYS_MS.length - 1)];
        return now + delay < deadline ? delay : -1;
    }

    // moonlight-common-c uses -1 for an unexpected ENet disconnect, -100 for
    // lost video traffic, and Android socket errno for failed send/receive.
    // Host termination reasons and local decoder failures are terminal.
    static boolean transportLost(int code) {
        switch (code) {
            case -1: case -100: case 32:
            case 100: case 101: case 102: case 103: case 104:
            case 107: case 110: case 111: case 112: case 113:
                return true;
            default:
                return false;
        }
    }
}
