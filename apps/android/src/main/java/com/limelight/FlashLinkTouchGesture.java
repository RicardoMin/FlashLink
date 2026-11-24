// SPDX-License-Identifier: MIT
package com.limelight;

/** Finger gestures in window coordinates. All input and timers run on the UI thread. */
final class FlashLinkTouchGesture {
    interface Output {
        void position(short x, short y, short width, short height);
        void button(boolean right, boolean down);
        void scroll(short amount);
    }

    interface Timer {
        long now();
        void at(long uptime, Runnable task);
        void remove(Runnable task);
    }

    // A polled game can miss a mouse-down and mouse-up sent in the same frame.
    private static final int CLICK_HOLD_MS = 50;
    private enum State { IDLE, TAP, POINT, LONG_PRESS, TWO_FINGERS, SCROLL }

    private final Output output;
    private final Timer timer;
    private final int slop, longPressMs;
    private final float wheelUnitsPerPx;
    private final Runnable longPress = this::longPress;
    private final Runnable release = this::releaseButton;
    private State state = State.IDLE;
    private int left, top, width, height;
    private int firstId, secondId;
    private float startX, startY, scrollY, wheelRemainder;
    private long longPressAt;
    private boolean buttonDown, rightButton;
    private short lastX, lastY, referenceWidth, referenceHeight;

    FlashLinkTouchGesture(Output output, Timer timer, int slop, int longPressMs, float density) {
        this.output = output;
        this.timer = timer;
        this.slop = slop;
        this.longPressMs = longPressMs;
        wheelUnitsPerPx = 120f / (32f * density); // One Windows wheel detent per 32 dp.
    }

    void viewport(int left, int top, int width, int height) {
        if (this.left == left && this.top == top && this.width == width && this.height == height) return;
        cancel();
        this.left = left;
        this.top = top;
        this.width = width;
        this.height = height;
        referenceWidth = (short)Math.min(width, Short.MAX_VALUE);
        referenceHeight = (short)Math.min(height, Short.MAX_VALUE);
    }

    private boolean inside(float x, float y) {
        return width > 1 && height > 1 && x >= left && y >= top && x < left + width && y < top + height;
    }

    private void position(float x, float y, boolean force) {
        // Moonlight normalizes each pixel against referenceSize - 1.
        short px = (short)Math.round(Math.min(width - 1, x - left) * (referenceWidth - 1) / (width - 1));
        short py = (short)Math.round(Math.min(height - 1, y - top) * (referenceHeight - 1) / (height - 1));
        if (force || px != lastX || py != lastY) {
            output.position(px, py, referenceWidth, referenceHeight);
            lastX = px;
            lastY = py;
        }
    }

    void down(int id, float x, float y, long time) {
        cancel();
        if (!inside(x, y)) return;
        state = State.TAP;
        firstId = id;
        startX = x;
        startY = y;
        position(x, y, true);
        longPressAt = time + longPressMs;
        timer.at(longPressAt, longPress);
    }

    void move(int id, float x, float y) {
        if (state != State.TAP && state != State.POINT) return;
        if (firstId != id || !inside(x, y)) {
            cancel();
            return;
        }
        float dx = x - startX, dy = y - startY;
        if (state == State.TAP && dx * dx + dy * dy > slop * slop) {
            timer.remove(longPress);
            state = State.POINT;
        }
        position(x, y, false);
    }

    void up(int id, float x, float y, long time) {
        move(id, x, y);
        timer.remove(longPress);
        if (state == State.TAP) {
            // Input and the long-press timer can both be waiting behind a busy UI thread.
            click(time >= longPressAt);
        }
        state = State.IDLE;
    }

    private void longPress() {
        if (state != State.TAP) return;
        state = State.LONG_PRESS;
        click(true);
    }

    private void click(boolean right) {
        // The host or a physical mouse may have moved the cursor since finger-down.
        output.position(lastX, lastY, referenceWidth, referenceHeight);
        buttonDown = true;
        rightButton = right;
        output.button(right, true);
        timer.at(timer.now() + CLICK_HOLD_MS, release);
    }

    private void releaseButton() {
        if (!buttonDown) return;
        buttonDown = false;
        output.button(rightButton, false);
    }

    void secondDown(int otherId, float x1, float y1, float x2, float y2) {
        if ((state != State.TAP && state != State.POINT) || otherId == firstId ||
                !inside(x1, y1) || !inside(x2, y2)) {
            cancel();
            return;
        }
        timer.remove(longPress);
        state = State.TWO_FINGERS;
        secondId = otherId;
        startX = (x1 + x2) * .5f;
        startY = (y1 + y2) * .5f;
        scrollY = startY;
        wheelRemainder = 0;
    }

    void twoMove(int id1, float x1, float y1, int id2, float x2, float y2) {
        if (state != State.TWO_FINGERS && state != State.SCROLL) return;
        if (!((id1 == firstId && id2 == secondId) || (id2 == firstId && id1 == secondId)) ||
                !inside(x1, y1) || !inside(x2, y2)) {
            cancel();
            return;
        }
        float centerY = (y1 + y2) * .5f;
        if (state == State.TWO_FINGERS) {
            float dx = (x1 + x2) * .5f - startX, dy = centerY - startY;
            if (Math.abs(dy) <= slop || Math.abs(dy) < Math.abs(dx)) return;
            state = State.SCROLL;
            // Scroll the window under the fingers without moving the pointer on every wheel event.
            position(startX, startY, true);
            scrollY = startY + Math.copySign(slop, dy);
        }
        float units = (centerY - scrollY) * wheelUnitsPerPx + wheelRemainder;
        short whole = (short)Math.max(Short.MIN_VALUE, Math.min(Short.MAX_VALUE, units));
        wheelRemainder = units - whole;
        scrollY = centerY;
        if (whole != 0) output.scroll(whole);
    }

    void cancel() {
        state = State.IDLE;
        timer.remove(longPress);
        timer.remove(release);
        releaseButton();
    }
}
