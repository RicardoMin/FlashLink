// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

public final class FlashLinkLiveSettingTest {
    private static final class Host implements FlashLinkLiveSetting.Listener {
        final FlashLinkLiveSetting control;
        Host() { this(FlashLinkLiveSetting.Kind.BITRATE); }
        Host(FlashLinkLiveSetting.Kind kind) { control = new FlashLinkLiveSetting(kind, this); }
        int id, target, effective, sends;
        FlashLinkLiveSetting.Result last;

        @Override public void send(int id, int kbps) {
            assert id != 0 && id != this.id;
            this.id = id;
            target = kbps;
            sends++;
        }
        @Override public void result(FlashLinkLiveSetting.Result result, int kbps) {
            last = result;
            effective = kbps;
        }
    }

    public static void main(String[] args) {
        Host host = new Host();
        host.control.probe();
        assert host.target == 0;
        assert !host.control.set(18000);
        host.control.response(host.id, 0, 0); // Malformed success isn't a capability grant.
        assert host.control.state() == FlashLinkLiveSetting.State.PROBING;
        host.control.response(host.id, 0, 10000);
        assert host.effective == 10000;
        assert !host.control.set(499) && !host.control.set(150001);
        assert host.control.set(24000);
        int firstSet = host.id;
        assert host.target == 24000 && !host.control.set(8000);
        host.control.response(firstSet, 0, 24000);
        assert host.last == FlashLinkLiveSetting.Result.APPLIED && host.effective == 24000;

        assert host.control.set(8000);
        int nextSet = host.id;
        host.control.response(firstSet, 0, 24000);
        assert host.control.state() == FlashLinkLiveSetting.State.SETTING;
        host.control.timeout(nextSet);
        assert host.target == 0 && host.id != nextSet;
        host.control.response(nextSet, 0, 8000);
        assert host.control.state() == FlashLinkLiveSetting.State.VERIFYING;
        host.control.response(host.id, 0, 8000);
        assert host.last == FlashLinkLiveSetting.Result.APPLIED && host.effective == 8000;
        host.control.timeout(host.id); // Stale timer.
        assert host.control.state() == FlashLinkLiveSetting.State.READY;

        assert host.control.set(13000);
        host.control.timeout(host.id);
        host.control.timeout(host.id);
        assert host.last == FlashLinkLiveSetting.Result.UNCONFIRMED;
        assert !host.control.set(15000);
        host.control.probe(); // Reopen the dialog after transport recovered.
        host.control.response(host.id, 0, 8000);
        assert host.last == FlashLinkLiveSetting.Result.REJECTED && host.effective == 8000;
        assert host.control.set(12000);
        host.control.response(host.id, 3, 8000);
        assert host.last == FlashLinkLiveSetting.Result.REJECTED && host.effective == 8000;
        assert host.control.set(9000);
        host.control.response(host.id, 1, 0);
        assert host.control.state() == FlashLinkLiveSetting.State.UNAVAILABLE;

        Host legacy = new Host();
        legacy.control.probe();
        legacy.control.timeout(legacy.id);
        legacy.control.response(legacy.id, 0, 10000);
        assert !legacy.control.set(8000) && legacy.sends == 1;
        Host stopping = new Host();
        stopping.control.probe();
        stopping.control.close();
        stopping.control.response(stopping.id, 0, 9000);
        stopping.control.timeout(stopping.id);
        assert stopping.last == null && stopping.sends == 1;
        Host fps = new Host(FlashLinkLiveSetting.Kind.FRAME_RATE);
        fps.control.probe();
        fps.control.response(fps.id, 0, 59);
        assert fps.effective == 59;
        assert !fps.control.set(241) && !fps.control.set(0);
        assert fps.control.set(30);
        fps.control.response(fps.id, 0, 30);
        assert fps.last == FlashLinkLiveSetting.Result.APPLIED;
        assert fps.control.set(120);
        int up = fps.id;
        fps.control.timeout(up);
        fps.control.response(up, 0, 120); // Stale SET after the recovery query.
        assert fps.control.state() == FlashLinkLiveSetting.State.VERIFYING;
        fps.control.response(fps.id, 0, 120);
        assert fps.last == FlashLinkLiveSetting.Result.APPLIED && fps.effective == 120;
        assert fps.control.set(240);
        fps.control.response(fps.id, 0, 120); // Host reports a different effective rate.
        assert fps.last == FlashLinkLiveSetting.Result.REJECTED && fps.effective == 120;
        System.out.println("Android live stream setting checks passed");
    }
}
