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
        FlashLinkQuality frameRate = new FlashLinkQuality(2280, 1080, 30, 12500);
        FlashLinkQuality bitrate = new FlashLinkQuality(2280, 1080, 59, 15000);
        assert previous.canApplyLive(frameRate, false, true, false);
        assert !previous.canApplyLive(frameRate, true, false, true);
        assert previous.canApplyLive(bitrate, true, false, false);
        assert !previous.canApplyLive(bitrate, false, true, true);
        assert !previous.canApplyLive(new FlashLinkQuality(2280, 1080, 30, 15000), true, true, true);
        FlashLinkQuality size = new FlashLinkQuality(1920, 1080, 59, 12500);
        assert !previous.canApplyLive(size, true, true, false);
        assert previous.canApplyLive(size, false, false, true);
        assert !previous.canApplyLive(new FlashLinkQuality(1920, 1080, 30, 12500), true, true, true);
        assert !previous.canApplyLive(new FlashLinkQuality(1920, 1080, 59, 15000), true, true, true);
        assert FlashLinkQuality.mbps(500).equals("0.5");
        assert FlashLinkQuality.mbps(150000).equals("150");
        assert FlashLinkQuality.fromArray(null) == null;
        assert FlashLinkQuality.fromArray(new int[]{1920, 1080, 60}) == null;
        assert FlashLinkQuality.fromArray(new int[]{1920, 1080, 60, 0}) == null;
        System.out.println("Android stream quality checks passed");
    }
}
