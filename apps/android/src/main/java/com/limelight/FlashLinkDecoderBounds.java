// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import java.util.ArrayList;
import java.util.List;
import java.util.function.BiPredicate;

/** Tries supported buffer sizes in descending order for each set of low-latency options. */
public final class FlashLinkDecoderBounds {
    private final List<Integer> sizes = new ArrayList<>();
    private int index, option;

    public FlashLinkDecoderBounds(int width, int height, int maxLongEdge, BiPredicate<Integer, Integer> supported) {
        if (supported != null) {
            for (int[] landscape : new int[][] {{4096, 2160}, {3840, 2160}, {2560, 1440}, {1920, 1080}}) {
                int w = width >= height ? landscape[0] : landscape[1];
                int h = width >= height ? landscape[1] : landscape[0];
                if (landscape[0] <= maxLongEdge && w >= width && h >= height && (w != width || h != height)
                        && supported.test(w, h)) sizes.add(FlashLinkResolution.pack(w, h));
            }
        }
        sizes.add(FlashLinkResolution.pack(width, height));
    }

    public int width() { return sizes.get(index) >>> 16; }
    public int height() { return sizes.get(index) & 0xffff; }
    public int option() { return option; }
    public boolean atStartupSize() { return index == sizes.size() - 1; }

    public boolean retry(boolean haveOtherOptions) {
        if (!atStartupSize()) {
            index++;
            return true;
        }
        if (!haveOtherOptions) return false;
        index = 0;
        option++;
        return true;
    }
}
