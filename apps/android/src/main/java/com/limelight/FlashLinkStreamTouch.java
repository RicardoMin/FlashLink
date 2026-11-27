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
    private boolean claimedTrackpadGesture;

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
        connection.sendMousePosition(x, y, width, height);
    }

    @Override public void button(boolean right, boolean down) {
        byte button = right ? MouseButtonPacket.BUTTON_RIGHT : MouseButtonPacket.BUTTON_LEFT;
        if (down) connection.sendMouseButtonDown(button);
        else connection.sendMouseButtonUp(button);
    }

    @Override public void scroll(short amount) {
        connection.sendMouseHighResScroll(amount);
    }

    void cancel() { gesture.cancel(); }
    void reset() { gesture.resetViewport(); }

    boolean onTouch(View source, MotionEvent event, boolean trackpadMode) {
        int action = event.getActionMasked();
        int count = event.getPointerCount();
        if (action == MotionEvent.ACTION_DOWN) claimedTrackpadGesture = false;
        if (action == MotionEvent.ACTION_CANCEL || (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0 || count > 2) {
            cancel();
            return !trackpadMode || claimedTrackpadGesture;
        }

        // Moonlight dispatches fingers to the stationary background sibling of the video.
        float dx = source.getLeft(), dy = source.getTop();
        int first = event.getPointerId(0);
        float x = event.getX() + dx, y = event.getY() + dy;
        switch (action) {
            case MotionEvent.ACTION_DOWN:
                gesture.down(first, x, y, event.getEventTime(), trackpadMode);
                break;
            case MotionEvent.ACTION_POINTER_DOWN:
                int added = event.getActionIndex(), existing = 1 - added;
                if (trackpadMode && !claimedTrackpadGesture) {
                    // The first DOWN also started Moonlight's native drag timer. Cancel it
                    // before the second DOWN can start a second native touch context.
                    game.cancelNativeTouchContexts();
                    claimedTrackpadGesture = true;
                }
                gesture.secondDown(event.getPointerId(added),
                        event.getX(existing) + dx, event.getY(existing) + dy,
                        event.getX(added) + dx, event.getY(added) + dy);
                break;
            case MotionEvent.ACTION_MOVE:
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
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
                    if (action == MotionEvent.ACTION_POINTER_UP) {
                        gesture.pointerUp(event.getPointerId(event.getActionIndex()));
                    }
                }
                break;
            default:
                break;
        }
        // Single-finger relative input stays native. A two-finger sequence stays here
        // through the last UP, including any remaining single-finger MOVE.
        return !trackpadMode || claimedTrackpadGesture;
    }
}
