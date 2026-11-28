// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import java.math.BigDecimal;

/** Video dimensions, negotiated frame rate and target stream bandwidth in kilobits per second. */
final class FlashLinkQuality {
    static final String EXTRA = "dev.flashlink.streamQuality";
    final int width, height, fps, bitrateKbps;

    FlashLinkQuality(int width, int height, int fps, int bitrateKbps) {
        if (width <= 0 || height <= 0 || fps <= 0 || bitrateKbps <= 0) {
            throw new IllegalArgumentException("Invalid stream quality");
        }
        this.width = width;
        this.height = height;
        this.fps = fps;
        this.bitrateKbps = bitrateKbps;
    }

    static FlashLinkQuality fromArray(int[] values) {
        if (values == null || values.length != 4) return null;
        for (int value : values) {
            if (value <= 0) return null;
        }
        return new FlashLinkQuality(values[0], values[1], values[2], values[3]);
    }

    int[] toArray() { return new int[] {width, height, fps, bitrateKbps}; }

    // Lower the rate before enlarging the picture. Raise it only after the new size is on screen.
    FlashLinkLiveSetting.Kind nextChange(FlashLinkQuality target) {
        if (target.fps < fps) return FlashLinkLiveSetting.Kind.FRAME_RATE;
        if (target.width != width || target.height != height) return FlashLinkLiveSetting.Kind.RESOLUTION;
        if (target.fps != fps) return FlashLinkLiveSetting.Kind.FRAME_RATE;
        if (target.bitrateKbps != bitrateKbps) return FlashLinkLiveSetting.Kind.BITRATE;
        return null;
    }

    String displayText() {
        return width + " × " + height + " · " + fps + " FPS · " + mbps(bitrateKbps) + " Mbps";
    }

    static String mbps(int kbps) {
        return BigDecimal.valueOf(kbps, 3).stripTrailingZeros().toPlainString();
    }

    @Override public boolean equals(Object obj) {
        if (!(obj instanceof FlashLinkQuality)) return false;
        FlashLinkQuality other = (FlashLinkQuality)obj;
        return width == other.width && height == other.height && fps == other.fps && bitrateKbps == other.bitrateKbps;
    }

    @Override public int hashCode() {
        return ((31 * width + height) * 31 + fps) * 31 + bitrateKbps;
    }
}
