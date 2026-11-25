// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

public final class FlashLinkQualityTest {
    public static void main(String[] args) {
        // The last working quality can use native dimensions and a display-capped FPS.
        // The activity-result round trip must preserve it exactly for rollback.
        FlashLinkQuality previous = new FlashLinkQuality(2280, 1080, 59, 12500);
        assert previous.equals(FlashLinkQuality.fromArray(previous.toArray()));
        assert !previous.equals(new FlashLinkQuality(2280, 1080, 59, 12000));
        assert !previous.equals(new FlashLinkQuality(2280, 1080, 60, 12500));
        assert !previous.equals(new FlashLinkQuality(1920, 1080, 59, 12500));
        assert previous.displayText().equals("2280 × 1080 · 59 FPS · 12.5 Mbps");
        assert FlashLinkQuality.mbps(500).equals("0.5");
        assert FlashLinkQuality.mbps(150000).equals("150");
        assert FlashLinkQuality.fromArray(null) == null;
        assert FlashLinkQuality.fromArray(new int[]{1920, 1080, 60}) == null;
        assert FlashLinkQuality.fromArray(new int[]{1920, 1080, 60, 0}) == null;
        System.out.println("Android stream quality checks passed");
    }
}
