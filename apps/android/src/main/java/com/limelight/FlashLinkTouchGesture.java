// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

/** Finger gestures in the stationary input container. All input and timers run on the UI thread. */
final class FlashLinkTouchGesture {
    interface Output {
        void position(short x, short y, short width, short height);
        void button(boolean right, boolean down);
        void scroll(short amount);
        void transform(float scale, float translationX, float translationY);
    }

    interface Timer {
        long now();
        void at(long uptime, Runnable task);
        void remove(Runnable task);
    }

    // A polled game can miss a mouse-down and mouse-up sent in the same frame.
    private static final int CLICK_HOLD_MS = 50;
    private static final int TWO_FINGER_TAP_MS = 320;
    private enum State { IDLE, TAP, POINT, LONG_PRESS, TWO_FINGERS, LAST_FINGER, SCROLL, PAN, PINCH }

    private final Output output;
    private final Timer timer;
    private final int slop, longPressMs;
    private final float trackpadSlop;
    private final float wheelUnitsPerPx;
    private final FlashLinkViewport view = new FlashLinkViewport();
    private final Runnable longPress = this::longPress;
    private final Runnable release = this::releaseButton;
    private State state = State.IDLE;
    private int firstId, secondId, remainingId;
    private float startX, startY, scrollY, wheelRemainder;
    private float previousX, previousY, startSpan, previousSpan;
    private float firstDownX, firstDownY, secondDownX, secondDownY;
    private boolean trackpad, twoFingerTapPossible;
    private long longPressAt, tapDeadline;
    private boolean buttonDown, rightButton;
    private short lastX, lastY;

    FlashLinkTouchGesture(Output output, Timer timer, int slop, int longPressMs, float density) {
        this.output = output;
        this.timer = timer;
        this.slop = slop;
        trackpadSlop = Math.max(slop, 16f * density);
        this.longPressMs = longPressMs;
        wheelUnitsPerPx = 120f / (32f * density); // One Windows wheel detent per 32 dp.
    }

    void viewport(int left, int top, int width, int height, int containerWidth, int containerHeight) {
        if (view.layout(left, top, width, height, containerWidth, containerHeight)) resetViewport();
    }

    void resetViewport() {
        cancel();
        view.reset();
        publishTransform();
    }

    private void publishTransform() {
        output.transform(view.scale(), view.translationX(), view.translationY());
    }

    private void position(float x, float y, boolean force) {
        if (trackpad) return;
        short px = view.remoteX(x), py = view.remoteY(y);
        if (force || px != lastX || py != lastY) {
            output.position(px, py, view.referenceWidth(), view.referenceHeight());
            lastX = px;
            lastY = py;
        }
    }

    void down(int id, float x, float y, long time, boolean trackpadMode) {
        cancel();
        trackpad = trackpadMode;
        if (!trackpad && !view.contains(x, y)) return;
        state = State.TAP;
        firstId = id;
        startX = x;
        startY = y;
        if (trackpad) {
            tapDeadline = time + TWO_FINGER_TAP_MS;
        } else {
            position(x, y, true);
            longPressAt = time + longPressMs;
            timer.at(longPressAt, longPress);
        }
    }

    void move(int id, float x, float y) {
        if (state == State.LAST_FINGER) {
            if (id != remainingId || leftTapArea(id, x, y)) cancel();
            return;
        }
        if (state != State.TAP && state != State.POINT) return;
        if (firstId != id || (!trackpad && !view.contains(x, y))) {
            cancel();
            return;
        }
        float dx = x - startX, dy = y - startY;
        if (state == State.TAP && pastSlop(dx, dy)) {
            timer.remove(longPress);
            state = State.POINT;
        }
        position(x, y, false);
    }

    void up(int id, float x, float y, long time) {
        move(id, x, y);
        timer.remove(longPress);
        if (state == State.LAST_FINGER && time <= tapDeadline) {
            click(true);
        } else if (!trackpad && state == State.TAP) {
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
        if (!trackpad) output.position(lastX, lastY, view.referenceWidth(), view.referenceHeight());
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
        if ((state != State.TAP && state != State.POINT && state != State.LONG_PRESS) || otherId == firstId ||
                (!trackpad && (!view.contains(x1, y1) || !view.contains(x2, y2)))) {
            cancel();
            return;
        }
        boolean firstFingerStill = state == State.TAP && !pastSlop(x1 - startX, y1 - startY);
        cancel(); // Also release a right press if the first finger waited for the second.
        state = State.TWO_FINGERS;
        twoFingerTapPossible = trackpad && firstFingerStill;
        secondId = otherId;
        firstDownX = trackpad ? startX : x1;
        firstDownY = trackpad ? startY : y1;
        secondDownX = x2;
        secondDownY = y2;
        previousX = startX = (x1 + x2) * .5f;
        previousY = startY = (y1 + y2) * .5f;
        previousSpan = startSpan = span(x1, y1, x2, y2);
        wheelRemainder = 0;
    }

    private float span(float x1, float y1, float x2, float y2) {
        return Math.max(Math.max(1, slop), (float)Math.hypot(x2 - x1, y2 - y1));
    }

    boolean isTransforming() { return state == State.PAN || state == State.PINCH; }
    boolean isTwoFingerPending() { return state == State.TWO_FINGERS; }

    private float movementSlop() { return trackpad ? trackpadSlop : slop; }

    private boolean pastSlop(float dx, float dy) {
        float limit = movementSlop();
        return dx * dx + dy * dy > limit * limit;
    }

    private boolean leftTapArea(int id, float x, float y) {
        if (id == firstId) return pastSlop(x - firstDownX, y - firstDownY);
        return pastSlop(x - secondDownX, y - secondDownY);
    }

    void pointerUp(int liftedId) {
        if (isTwoFingerPending() && twoFingerTapPossible && (liftedId == firstId || liftedId == secondId)) {
            remainingId = liftedId == firstId ? secondId : firstId;
            state = State.LAST_FINGER;
        } else {
            cancel();
        }
    }

    void twoMove(int id1, float x1, float y1, int id2, float x2, float y2) {
        if (!(isTwoFingerPending() || isTransforming() || state == State.SCROLL)) return;
        if (!((id1 == firstId && id2 == secondId) || (id2 == firstId && id1 == secondId))) {
            cancel();
            return;
        }
        float centerX = (x1 + x2) * .5f, centerY = (y1 + y2) * .5f;
        float span = span(x1, y1, x2, y2);
        float spanChange = Math.abs(span - startSpan);
        if (isTwoFingerPending()) {
            // Opposing fingertip jitter adds up in the span. Wait for one finger to leave
            // its tap area, and remember an excursion even if the fingers move back.
            if (twoFingerTapPossible) {
                if (!leftTapArea(id1, x1, y1) && !leftTapArea(id2, x2, y2)) return;
                twoFingerTapPossible = false;
            }
            float dx = centerX - startX, dy = centerY - startY;
            float threshold = movementSlop();
            if (spanChange > threshold && (view.scale() > 1f || spanChange >= Math.abs(dy))) {
                state = State.PINCH;
            } else if (view.scale() > 1f && pastSlop(dx, dy)) {
                state = State.PAN;
            } else if (view.scale() == 1f && Math.abs(dy) > threshold && Math.abs(dy) >= Math.abs(dx)) {
                state = State.SCROLL;
                // Scroll the window under the fingers without moving the pointer on every wheel event.
                position(startX, startY, true);
                scrollY = startY + Math.copySign(threshold, dy);
            } else {
                return;
            }
        }
        if (isTransforming()) {
            if (state == State.PAN && spanChange > movementSlop()) state = State.PINCH;
            float factor = state == State.PINCH ? span / previousSpan : 1f;
            view.transform(previousX, previousY, centerX, centerY, factor);
            publishTransform();
            previousX = centerX;
            previousY = centerY;
            if (state == State.PINCH) previousSpan = span;
            return;
        }
        if (!trackpad && (!view.contains(x1, y1) || !view.contains(x2, y2))) {
            cancel();
            return;
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
