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
        assert previous.nextChange(frameRate) == FlashLinkLiveSetting.Kind.FRAME_RATE;
        assert previous.nextChange(bitrate) == FlashLinkLiveSetting.Kind.BITRATE;
        FlashLinkQuality target = new FlashLinkQuality(1920, 1080, 30, 15000);
        assert previous.nextChange(target) == FlashLinkLiveSetting.Kind.FRAME_RATE;
        assert frameRate.nextChange(target) == FlashLinkLiveSetting.Kind.RESOLUTION;
        assert new FlashLinkQuality(1920, 1080, 30, 12500).nextChange(target) == FlashLinkLiveSetting.Kind.BITRATE;
        assert target.nextChange(target) == null;
        assert target.nextChange(previous) == FlashLinkLiveSetting.Kind.RESOLUTION;
        assert FlashLinkQuality.mbps(500).equals("0.5");
        assert FlashLinkQuality.mbps(150000).equals("150");
        assert FlashLinkQuality.fromArray(null) == null;
        assert FlashLinkQuality.fromArray(new int[]{1920, 1080, 60}) == null;
        assert FlashLinkQuality.fromArray(new int[]{1920, 1080, 60, 0}) == null;
        System.out.println("Android stream quality checks passed");
    }
}
