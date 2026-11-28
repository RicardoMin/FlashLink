// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

public final class FlashLinkFrameRateTest {
    public static void main(String[] args) {
        assert FlashLinkFrameRate.surfaceVote(30, 120, false) == 120;
        assert FlashLinkFrameRate.surfaceVote(60, 120, false) == 120;
        assert FlashLinkFrameRate.surfaceVote(30, 120, true) == 30;
        assert FlashLinkFrameRate.surfaceVote(120, 60, false) == 120;
        assert FlashLinkFrameRate.surfaceVote(120, 119.88f, false) == 120;

        FlashLinkFrameRate session = new FlashLinkFrameRate();
        session.sendRequest(0, () -> true); // The capability probe also runs against older hosts.
        assert session.avcLevelIdc(51, 1920, 1080, 30, false) == 42;
        assert session.avcLevelIdc(51, 1280, 720, 30, false) == 32;
        assert session.avcLevelIdc(51, 1920, 1080, 30, true) == 51;

        boolean sent = session.sendRequest(120, () -> {
            // New high-rate IDR and SPS on the media channel, still showing 30 FPS locally.
            assert session.avcLevelIdc(51, 1920, 1080, 30, false) == 51;
            return true;
        });
        assert sent;
        assert session.avcLevelIdc(51, 1920, 1080, 120, false) == 51;
        session.sendRequest(30, () -> true);
        assert session.avcLevelIdc(50, 1280, 720, 30, false) == 50;
        assert new FlashLinkFrameRate().avcLevelIdc(50, 1280, 720, 30, false) == 32;
        System.out.println("Android frame rate rendering checks passed");
    }
}
