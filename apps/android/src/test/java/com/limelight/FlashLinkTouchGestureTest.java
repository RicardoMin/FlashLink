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
        float scale, translationX, translationY;

        Fixture() { touch.viewport(50, 100, 1000, 600, 1100, 800); }
        public void transform(float scale, float x, float y) {
            this.scale = scale;
            translationX = x;
            translationY = y;
        }
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

        void expectTransform(float zoom, float x, float y) {
            if (Math.abs(scale - zoom) > .001f || Math.abs(translationX - x) > .01f ||
                    Math.abs(translationY - y) > .01f)
                throw new AssertionError("Unexpected transform " + scale + " " + translationX + " " + translationY);
        }

        void startTwo(float x1, float x2, float y) {
            touch.down(9, x1, y, clock);
            if (events.size() != 1 || !events.remove(0).startsWith("pos ")) throw new AssertionError(events);
            touch.secondDown(2, x1, y, x2, y);
            if (!touch.isTwoFingerPending()) throw new AssertionError("Second contact was ignored");
        }

        void endTwo() {
            if (!touch.isTransforming()) throw new AssertionError("Viewport gesture lost before lift");
            touch.cancel(); // First pointer up, then a move and lift from the remaining pointer.
            touch.move(9, 550, 390);
            touch.up(9, 550, 390, clock);
            advance(clock + 600);
            expect();
        }

        void tap(float x, float y, String expectedPosition) {
            touch.down(1, x, y, clock);
            touch.up(1, x, y, clock + 20);
            advance(clock + 80);
            expect(expectedPosition, expectedPosition, "left down", "left up");
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
        f.touch.viewport(5, 10, 600, 1000, 610, 1020);
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

    private static void pinchPanAndTap() {
        Fixture f = new Fixture();
        f.startTwo(400, 600, 360);
        f.touch.twoMove(2, 603, 359, 9, 398, 361);
        if (!f.touch.isTwoFingerPending()) throw new AssertionError("Sub-slop jitter claimed the gesture");
        f.touch.twoMove(9, 300, 360, 2, 700, 360);
        f.endTwo();
        f.expectTransform(2, -450, -260);
        // At 2x this off-center pinch keeps the original video point below the midpoint.
        f.tap(500, 360, "pos 900 520 2000 1200");
        for (int xDir : new int[]{1, -1}) {
            for (int yDir : new int[]{1, -1}) {
                for (int i = 0; i < 3; i++) {
                    f.startTwo(520, 580, 400);
                    f.touch.twoMove(2, 580 + xDir * 440, 400 + yDir * 300,
                                    9, 520 + xDir * 440, 400 + yDir * 300);
                    f.endTwo();
                }
                f.expectTransform(2, xDir > 0 ? -50 : -950, yDir > 0 ? -100 : -500);
                f.tap(xDir > 0 ? 0 : 1099, yDir > 0 ? 0 : 799,
                      "pos " + (xDir > 0 ? 0 : 1999) + " " + (yDir > 0 ? 0 : 1199) + " 2000 1200");
            }
        }
        f.startTwo(500, 600, 400);
        f.touch.resetViewport();
        f.touch.up(9, 550, 400, f.clock);
        f.advance(f.clock + 600);
        f.expect();
        f.expectTransform(1, 0, 0);
        f.tap(50, 100, "pos 0 0 1000 600");
        f.tap(1049, 699, "pos 999 599 1000 600");
        f.touch.down(1, 0, 0, f.clock);
        f.touch.up(1, 0, 0, f.clock + 10);
        f.expect();
    }

    private static void pinchLimitsAndLayout() {
        Fixture f = new Fixture();
        f.startTwo(500, 540, 400);
        f.touch.twoMove(9, 120, 400, 2, 920, 400); // Overshoot the 4x limit.
        f.expectTransform(4, -1410, -900);
        f.touch.twoMove(9, 320, 400, 2, 720, 400); // Start shrinking immediately.
        f.expectTransform(2, -470, -300);
        f.touch.twoMove(9, 510, 400, 2, 530, 400);
        f.expectTransform(1, 0, 0);
        f.touch.twoMove(9, 510, 500, 2, 530, 500); // Still a local gesture at minimum scale.
        f.endTwo();
        f.startTwo(420, 620, 400);
        f.touch.twoMove(9, 400, 420, 2, 640, 420); // Enlarged video is still letterboxed vertically.
        f.endTwo();
        f.expectTransform(1.2f, -94, -60);
        f.touch.down(1, 500, 39, f.clock);
        f.touch.up(1, 500, 39, f.clock);
        f.expect();
        f.startTwo(460, 580, 400);
        f.touch.viewport(0, 250, 600, 300, 600, 800); // Rotation or stream aspect ratio change.
        f.touch.twoMove(9, 300, 350, 2, 550, 350);
        f.touch.up(9, 300, 350, f.clock);
        f.expect();
        f.expectTransform(1, 0, 0);
        f.tap(300, 400, "pos 300 150 600 300");
    }

    private static void pinchAfterLongPress() {
        Fixture f = new Fixture();
        f.touch.down(9, 450, 400, 0);
        f.advance(500);
        f.expect("pos 400 300 1000 600", "pos 400 300 1000 600", "right down");
        f.touch.secondDown(2, 450, 400, 650, 400);
        f.expect("right up");
        f.touch.twoMove(9, 350, 400, 2, 750, 400);
        f.endTwo();
        f.expectTransform(2, -500, -300);
    }

    private static void trackpadTapSlop() {
        Fixture f = new Fixture();
        f.startTwo(500, 600, 400);
        f.touch.twoMove(2, 601, 399, 9, 499, 401);
        if (f.touch.isDisplacedTwoFingerPending()) throw new AssertionError("Jitter swallowed a trackpad tap");
        f.touch.twoMove(2, 700, 400, 9, 600, 400);
        f.touch.twoMove(9, 500, 400, 2, 600, 400);
        if (!f.touch.isDisplacedTwoFingerPending()) throw new AssertionError("Sweep became a trackpad tap");
        f.touch.cancel();
        f.startTwo(500, 600, 400);
        if (f.touch.isDisplacedTwoFingerPending()) throw new AssertionError("New tap inherited the sweep");
        f.touch.twoMove(9, 550, 350, 2, 550, 450); // Rotation with stationary midpoint and constant span.
        if (!f.touch.isDisplacedTwoFingerPending()) throw new AssertionError("Rotation became a trackpad tap");
    }

    public static void main(String[] args) {
        coordinatesAndBars();
        tapsAndMovement();
        longPressAndCancel();
        twoFingers();
        pinchPanAndTap();
        pinchLimitsAndLayout();
        pinchAfterLongPress();
        trackpadTapSlop();
        System.out.println("Touch gesture tests passed");
    }
}
