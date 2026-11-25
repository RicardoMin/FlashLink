// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.input.MouseButtonPacket;

/** Keeps the video transform and touch mapping in the coordinate system of the fixed input layer. */
final class FlashLinkStreamTouch implements FlashLinkTouchGesture.Output {
    private final FlashLinkGame game;
    private final View video, container;
    private final NvConnection connection;
    private final FlashLinkTouchGesture gesture;
    private boolean trackpad, claimedTrackpadGesture;

    FlashLinkStreamTouch(FlashLinkGame game, View video, NvConnection connection) {
        this.game = game;
        this.video = video;
        this.container = (View)video.getParent();
        this.connection = connection;
        video.setPivotX(0);
        video.setPivotY(0);
        Handler handler = new Handler(Looper.getMainLooper());
        gesture = new FlashLinkTouchGesture(this, new FlashLinkTouchGesture.Timer() {
            public long now() { return SystemClock.uptimeMillis(); }
            public void at(long time, Runnable task) { handler.postAtTime(task, time); }
            public void remove(Runnable task) { handler.removeCallbacks(task); }
        }, ViewConfiguration.get(game).getScaledTouchSlop(), ViewConfiguration.getLongPressTimeout(),
                video.getResources().getDisplayMetrics().density);
        View.OnLayoutChangeListener layout = (v, l, t, r, b, oldL, oldT, oldR, oldB) -> updateViewport();
        video.addOnLayoutChangeListener(layout);
        container.addOnLayoutChangeListener(layout);
        updateViewport();
    }

    private void updateViewport() {
        gesture.viewport(video.getLeft(), video.getTop(), video.getWidth(), video.getHeight(),
                container.getWidth(), container.getHeight());
    }

    @Override public void transform(float scale, float translationX, float translationY) {
        video.setScaleX(scale);
        video.setScaleY(scale);
        video.setTranslationX(translationX);
        video.setTranslationY(translationY);
        game.showViewReset(scale > 1f);
    }

    @Override public void position(short x, short y, short width, short height) {
        if (!trackpad) connection.sendMousePosition(x, y, width, height);
    }

    @Override public void button(boolean right, boolean down) {
        if (trackpad) return;
        byte button = right ? MouseButtonPacket.BUTTON_RIGHT : MouseButtonPacket.BUTTON_LEFT;
        if (down) connection.sendMouseButtonDown(button);
        else connection.sendMouseButtonUp(button);
    }

    @Override public void scroll(short amount) {
        claimTrackpadGesture();
        connection.sendMouseHighResScroll(amount);
    }

    private void claimTrackpadGesture() {
        if (trackpad && !claimedTrackpadGesture) {
            game.cancelNativeTouchContexts();
            claimedTrackpadGesture = true;
        }
    }

    void cancel() { gesture.cancel(); }
    void reset() { gesture.resetViewport(); }

    boolean onTouch(View source, MotionEvent event, boolean trackpadMode) {
        int action = event.getActionMasked();
        int count = event.getPointerCount();
        trackpad = trackpadMode;
        if (action == MotionEvent.ACTION_DOWN) claimedTrackpadGesture = false;
        if (action == MotionEvent.ACTION_CANCEL || (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0 || count > 2) {
            cancel();
            return !trackpad || claimedTrackpadGesture;
        }

        // Moonlight dispatches fingers to the stationary background sibling of the video.
        float dx = source.getLeft(), dy = source.getTop();
        int first = event.getPointerId(0);
        float x = event.getX() + dx, y = event.getY() + dy;
        switch (action) {
            case MotionEvent.ACTION_DOWN:
                gesture.down(first, x, y, event.getEventTime());
                break;
            case MotionEvent.ACTION_POINTER_DOWN:
                int added = event.getActionIndex(), existing = 1 - added;
                gesture.secondDown(event.getPointerId(added),
                        event.getX(existing) + dx, event.getY(existing) + dy,
                        event.getX(added) + dx, event.getY(added) + dy);
                break;
            case MotionEvent.ACTION_MOVE:
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                boolean pending = gesture.isTwoFingerPending();
                for (int i = 0; i < event.getHistorySize(); i++) {
                    float hx = event.getHistoricalX(0, i) + dx, hy = event.getHistoricalY(0, i) + dy;
                    if (count == 1) gesture.move(first, hx, hy);
                    else gesture.twoMove(first, hx, hy, event.getPointerId(1),
                            event.getHistoricalX(1, i) + dx, event.getHistoricalY(1, i) + dy);
                }
                if (action == MotionEvent.ACTION_UP) {
                    gesture.up(first, x, y, event.getEventTime());
                } else if (count == 1) {
                    gesture.move(first, x, y);
                } else {
                    gesture.twoMove(first, x, y, event.getPointerId(1), event.getX(1) + dx, event.getY(1) + dy);
                    boolean movedWithoutScrolling = action == MotionEvent.ACTION_POINTER_UP &&
                            gesture.isDisplacedTwoFingerPending();
                    if ((pending && !gesture.isTwoFingerPending()) || movedWithoutScrolling) claimTrackpadGesture();
                    if (action == MotionEvent.ACTION_POINTER_UP) cancel();
                }
                break;
            default:
                break;
        }
        // Native trackpad owns taps. Once a wheel or viewport gesture wins, also consume
        // the final finger so that Moonlight cannot turn the release into a right click.
        return !trackpad || claimedTrackpadGesture ||
                (action == MotionEvent.ACTION_MOVE && gesture.isTwoFingerPending());
    }
}
