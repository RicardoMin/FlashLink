// SPDX-License-Identifier: MIT
package com.limelight;

import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;

/** Adapts finger events from the background input view to the displayed video rectangle. */
final class FlashLinkDirectTouch {
    private final View video;
    private final FlashLinkTouchGesture gesture;
    private final int[] videoOrigin = new int[2], sourceOrigin = new int[2];

    FlashLinkDirectTouch(View video, FlashLinkTouchGesture.Output sink) {
        this.video = video;
        Handler handler = new Handler(Looper.getMainLooper());
        gesture = new FlashLinkTouchGesture(sink, new FlashLinkTouchGesture.Timer() {
            public long now() { return SystemClock.uptimeMillis(); }
            public void at(long time, Runnable task) { handler.postAtTime(task, time); }
            public void remove(Runnable task) { handler.removeCallbacks(task); }
        }, ViewConfiguration.get(video.getContext()).getScaledTouchSlop(),
                ViewConfiguration.getLongPressTimeout(), video.getResources().getDisplayMetrics().density);
        video.addOnLayoutChangeListener((v, l, t, r, b, oldL, oldT, oldR, oldB) -> updateViewport());
    }

    private void updateViewport() {
        video.getLocationInWindow(videoOrigin);
        gesture.viewport(videoOrigin[0], videoOrigin[1], video.getWidth(), video.getHeight());
    }

    void cancel() { gesture.cancel(); }

    void onTouch(View source, MotionEvent event) {
        int action = event.getActionMasked();
        int count = event.getPointerCount();
        if (action == MotionEvent.ACTION_CANCEL || (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0 || count > 2) {
            cancel();
            return;
        }
        updateViewport();
        source.getLocationInWindow(sourceOrigin);
        int dx = sourceOrigin[0], dy = sourceOrigin[1];
        int first = event.getPointerId(0);
        float x = event.getX() + dx, y = event.getY() + dy;
        switch (action) {
            case MotionEvent.ACTION_DOWN:
                gesture.down(first, x, y, event.getEventTime());
                break;
            case MotionEvent.ACTION_POINTER_DOWN:
                gesture.secondDown(event.getPointerId(event.getActionIndex()), x, y,
                        event.getX(1) + dx, event.getY(1) + dy);
                break;
            case MotionEvent.ACTION_MOVE:
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                // Keep historical samples for the slop decision even when Android batches movement.
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
                    if (action == MotionEvent.ACTION_POINTER_UP) cancel();
                }
                break;
            default:
                break;
        }
    }
}
