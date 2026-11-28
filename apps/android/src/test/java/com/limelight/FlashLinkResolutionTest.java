// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

public final class FlashLinkResolutionTest {
    private static final int HD = FlashLinkResolution.pack(1280, 720);
    private static final int FULL_HD = FlashLinkResolution.pack(1920, 1080);
    private static final FlashLinkLiveSetting.Result ACK = FlashLinkLiveSetting.Result.APPLIED;

    private static final class Host implements FlashLinkResolution.Listener {
        final FlashLinkResolution size = new FlashLinkResolution(this);
        int target, saved, restored, failures;
        @Override public boolean request(int value) { target = value; return true; }
        @Override public void finished(int value, boolean remember) {
            if (remember) saved = value;
            else restored = value;
        }
        @Override public void unconfirmed() { failures++; }
    }

    public static void main(String[] args) {
        // This AVC decoder rejects option 0 at every size. Option 1 must get
        // another chance with 4K bounds even after trying 720p under option 0.
        FlashLinkDecoderBounds bounds = new FlashLinkDecoderBounds(1280, 720, 3840, (w, h) -> true);
        for (int width : new int[]{3840, 2560, 1920, 1280}) {
            assert bounds.option() == 0 && bounds.width() == width;
            assert bounds.retry(true);
        }
        assert bounds.option() == 1 && bounds.width() == 3840;
        // A codec without adaptive playback tries its startup geometry once per option.
        FlashLinkDecoderBounds fixed = new FlashLinkDecoderBounds(1280, 720, 0, null);
        assert fixed.atStartupSize() && fixed.retry(true) && fixed.width() == 1280;
        assert fixed.option() == 1 && !fixed.retry(false);

        assert FlashLinkResolution.valid(FULL_HD);
        assert !FlashLinkResolution.valid(0);
        assert !FlashLinkResolution.valid(FlashLinkResolution.pack(1921, 1080));
        assert !FlashLinkResolution.valid(FlashLinkResolution.pack(4096, 4096));
        Host host = new Host();
        assert host.size.begin(HD, FULL_HD);
        host.size.response(ACK, FULL_HD);
        assert host.size.pending() && host.saved == 0;
        host.size.presented(FULL_HD);
        assert host.saved == FULL_HD && !host.size.pending();
        assert host.size.begin(FULL_HD, HD);
        host.size.presented(HD);
        assert host.size.pending() && host.saved == FULL_HD;
        host.size.response(ACK, HD);
        assert host.saved == HD;
        host.size.inputTooSmall(FULL_HD); // A delayed callback from the previous geometry.
        assert host.failures == 0 && !host.size.pending();
        host.size.inputTooSmall(HD); // A later keyframe exceeds the codec's actual input capacity.
        assert host.target == FULL_HD && host.size.pending();
        host.size.response(ACK, FULL_HD);
        host.size.presented(FULL_HD);
        assert host.saved == FULL_HD && !host.size.pending();

        host = new Host();
        assert host.size.begin(HD, FULL_HD);
        int token = host.size.generation();
        host.size.response(ACK, FULL_HD); // Missing output triggers a reverse request.
        host.size.timeout(token);
        assert host.target == HD && host.size.pending();
        host.size.timeout(token);
        assert host.failures == 0;
        host.size.response(ACK, HD);
        assert host.size.pending() && host.restored == 0;
        host.size.presented(HD);
        assert host.saved == 0 && host.restored == HD && !host.size.pending();

        assert host.size.begin(HD, FULL_HD);
        host.size.presented(FULL_HD);
        host.size.timeout(host.size.generation());
        host.size.timeout(host.size.generation()); // In-session rollback also failed.
        assert host.saved == 0 && host.failures == 1 && !host.size.pending();
        assert host.size.begin(HD, FULL_HD);
        host.size.response(FlashLinkLiveSetting.Result.REJECTED, HD);
        assert !host.size.pending() && host.saved == 0;
        host.size.close();
        host.size.response(ACK, FULL_HD);
        assert host.saved == 0;
        System.out.println("Android resolution confirmation checks passed");
    }
}
