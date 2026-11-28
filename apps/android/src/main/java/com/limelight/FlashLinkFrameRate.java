// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import java.util.function.BooleanSupplier;

/** Frame-rate decisions shared by the display and the decoder for one stream. */
public final class FlashLinkFrameRate {
    private volatile boolean liveChangeRequested;

    boolean sendRequest(int fps, BooleanSupplier send) {
        // A new SPS can reach the decoder before the control reply, or even before send returns.
        if (fps > 0) liveChangeRequested = true;
        return send.getAsBoolean();
    }

    static float surfaceVote(int fps, float preferredRefreshRate, boolean mayReduceRefreshRate) {
        return mayReduceRefreshRate ? fps : Math.max(preferredRefreshRate, fps);
    }

    public int avcLevelIdc(int signaled, int width, int height, int fps, boolean refFrameInvalidation) {
        if (liveChangeRequested || refFrameInvalidation || fps > 60) return signaled;
        // Moonlight's fixed-rate optimization limits decoder buffering for these profiles.
        if (width <= 720 && height <= 480) return 31;
        if (width <= 1280 && height <= 720) return 32;
        if (width <= 1920 && height <= 1080) return 42;
        return signaled;
    }
}
