// SPDX-License-Identifier: MIT
package com.limelight;

/** Host-JVM regression test for the exact recovery policy used by the Android APK. */
public final class FlashLinkReconnectTest {
    public static void main(String[] args) {
        FlashLinkReconnect retry = new FlashLinkReconnect();
        retry.interrupted(1000, 0);
        assert retry.deadline == 16000;
        long now = 1000;
        for (long expected : new long[]{750, 1500, 3000, 4000, 4000}) {
            long delay = retry.nextDelay(now);
            assert delay == expected;
            now += delay;
            retry.attempts++;
        }
        assert retry.nextDelay(now) == -1;
        // A second interruption during the same recovery must not extend the deadline.
        retry.interrupted(15000, 16000);
        assert retry.nextDelay(15000) == 750;
        assert retry.nextDelay(15250) == -1;
        assert retry.nextDelay(16500) == -1;
        for (int code : new int[]{-1, -100, 101, 104, 110, 113}) {
            assert FlashLinkReconnect.transportLost(code);
        }
        for (int code : new int[]{0, -101, -102, -103, -104, 401, 403, 0x80030023}) {
            assert !FlashLinkReconnect.transportLost(code);
        }
        System.out.println("Android recovery policy passed");
    }
}
