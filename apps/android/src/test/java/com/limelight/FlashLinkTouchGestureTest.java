// SPDX-License-Identifier: MIT
package com.limelight;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public final class FlashLinkTouchGestureTest {
    private static final class Fixture implements FlashLinkTouchGesture.Output, FlashLinkTouchGesture.Timer {
        final List<String> events = new ArrayList<>();
        final Map<Runnable, Long> pending = new LinkedHashMap<>();
        final FlashLinkTouchGesture touch = new FlashLinkTouchGesture(this, this, 8, 500, 2);
        long clock;

        Fixture() { touch.viewport(50, 100, 1000, 600); }
        public void position(short x, short y, short w, short h) {
            events.add("pos " + x + " " + y + " " + w + " " + h);
        }
        public void button(boolean right, boolean down) {
            events.add((right ? "right " : "left ") + (down ? "down" : "up"));
        }
        public void scroll(short amount) { events.add("wheel " + amount); }
        public long now() { return clock; }
        public void at(long time, Runnable task) { pending.put(task, time); }
        public void remove(Runnable task) { pending.remove(task); }

        void advance(long time) {
            while (!pending.isEmpty()) {
                Map.Entry<Runnable, Long> next = null;
                for (Map.Entry<Runnable, Long> entry : pending.entrySet()) {
                    if (next == null || entry.getValue() < next.getValue()) next = entry;
                }
                if (next.getValue() > time) break;
                clock = next.getValue();
                Runnable task = next.getKey();
                pending.remove(task);
                task.run();
            }
            clock = time;
        }

        void expect(String... expected) {
            if (!events.equals(Arrays.asList(expected))) throw new AssertionError(events + " != " + Arrays.toString(expected));
            events.clear();
        }
    }

    private static void coordinatesAndBars() {
        for (int[] xy : new int[][]{{0, 0}, {999, 0}, {0, 599}, {999, 599}, {500, 300}}) {
            Fixture f = new Fixture();
            int x = xy[0] + 50, y = xy[1] + 100;
            f.touch.down(7, x, y, 0);
            String position = "pos " + xy[0] + " " + xy[1] + " 1000 600";
            f.expect(position);
            f.advance(24);
            f.touch.up(7, x, y, 24);
            f.expect(position, "left down");
            f.advance(73);
            f.expect();
            f.advance(74);
            f.expect("left up");
        }
        for (float[] xy : new float[][]{{49.9f, 500}, {1050, 500}, {500, 99.9f}, {500, 700}}) {
            Fixture f = new Fixture();
            f.touch.down(1, xy[0], xy[1], 0);
            f.touch.move(1, 550, 400);
            f.touch.up(1, 550, 400, 30);
            f.advance(1000);
            f.expect();
        }
        Fixture f = new Fixture();
        f.touch.down(1, 550, 400, 0);
        f.touch.move(1, 1100, 400);
        f.touch.up(1, 550, 400, 30);
        f.advance(600);
        f.expect("pos 500 300 1000 600");
    }

    private static void tapsAndMovement() {
        Fixture f = new Fixture();
        f.touch.down(3, 149, 199, 0);
        f.advance(30);
        f.touch.up(3, 150, 200, 30);
        f.advance(50);  // Rapid second tap overlaps the first release timer.
        f.touch.down(4, 150, 200, 50);
        f.advance(70);
        f.touch.up(4, 150, 200, 70);
        f.advance(200);
        f.expect("pos 99 99 1000 600", "pos 100 100 1000 600", "pos 100 100 1000 600", "left down", "left up",
                 "pos 100 100 1000 600", "pos 100 100 1000 600", "left down", "left up");
        f.touch.down(5, 550, 400, 200);
        f.touch.move(5, 600, 400);
        f.touch.move(5, 550, 400);
        f.advance(1000);
        f.touch.up(5, 550, 400, 1000);
        f.expect("pos 500 300 1000 600", "pos 550 300 1000 600", "pos 500 300 1000 600");
    }

    private static void longPressAndCancel() {
        Fixture f = new Fixture();
        f.touch.down(1, 550, 400, 0);
        f.advance(499);
        f.expect("pos 500 300 1000 600");
        f.advance(500);
        f.expect("pos 500 300 1000 600", "right down");
        f.touch.up(1, 550, 400, 501);
        f.advance(700);
        f.expect("right up");
        f.touch.down(1, 550, 400, 700);
        f.touch.cancel(); // Mode change, focus loss, interrupted stream or palm rejection.
        f.advance(1500);
        f.touch.up(1, 550, 400, 1500);
        f.expect("pos 500 300 1000 600");
        f.touch.down(2, 550, 400, 1500);
        f.touch.up(2, 550, 400, 1510);
        f.touch.cancel();
        f.advance(2000);
        f.expect("pos 500 300 1000 600", "pos 500 300 1000 600", "left down", "left up");
        f.touch.down(3, 550, 400, 2000);
        f.touch.viewport(5, 10, 600, 1000);
        f.advance(3000);
        f.touch.up(3, 550, 400, 3000);
        f.expect("pos 500 300 1000 600");
    }

    private static void twoFingers() {
        Fixture f = new Fixture();
        f.touch.down(9, 420, 400, 0);
        f.touch.secondDown(2, 420, 400, 460, 400);
        f.touch.twoMove(2, 460, 402, 9, 420, 402); // New index order and slight jitter.
        f.advance(600);
        f.expect("pos 370 300 1000 600");
        f.touch.twoMove(2, 460, 328, 9, 420, 328);
        f.expect("pos 390 300 1000 600", "wheel -120"); // Slop 8 px, then one detent at 2x.
        f.touch.twoMove(9, 420, 392, 2, 460, 392);
        f.expect("wheel 120");
        f.touch.cancel(); // Either finger lifted. Ignore the remaining finger until a new down.
        f.touch.move(9, 420, 393);
        f.touch.up(9, 420, 393, 620);
        f.expect();
        f.touch.down(1, 550, 400, 700);
        f.touch.secondDown(2, 550, 400, 560, 400);
        f.touch.cancel(); // Two-finger tap; no left or right click.
        f.touch.up(1, 550, 400, 720);
        f.advance(1500);
        f.expect("pos 500 300 1000 600");
    }

    public static void main(String[] args) {
        coordinatesAndBars();
        tapsAndMovement();
        longPressAndCancel();
        twoFingers();
        System.out.println("Touch gesture tests passed");
    }
}
