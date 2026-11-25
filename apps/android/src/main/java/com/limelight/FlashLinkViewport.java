// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

/** The video rectangle and its client-side transform, in the input container's coordinates. */
final class FlashLinkViewport {
    private static final float MAX_SCALE = 4f;
    private int left, top, width, height, containerWidth, containerHeight;
    private float scale = 1f, translationX, translationY;

    boolean layout(int left, int top, int width, int height, int containerWidth, int containerHeight) {
        if (this.left == left && this.top == top && this.width == width && this.height == height &&
                this.containerWidth == containerWidth && this.containerHeight == containerHeight) return false;
        this.left = left;
        this.top = top;
        this.width = width;
        this.height = height;
        this.containerWidth = containerWidth;
        this.containerHeight = containerHeight;
        return true;
    }

    void reset() {
        scale = 1f;
        translationX = translationY = 0f;
    }

    float scale() { return scale; }
    float translationX() { return translationX; }
    float translationY() { return translationY; }

    boolean contains(float x, float y) {
        float originX = left + translationX, originY = top + translationY;
        return width > 1 && height > 1 && x >= 0 && y >= 0 && x < containerWidth && y < containerHeight &&
                x >= originX && x < originX + width * scale && y >= originY && y < originY + height * scale;
    }

    void transform(float fromX, float fromY, float toX, float toY, float factor) {
        float nextScale = Math.max(1f, Math.min(MAX_SCALE, scale * factor));
        if (nextScale == 1f) {
            reset();
            return;
        }
        // Keep the video point under the old midpoint beneath the new one, until an edge is reached.
        float ratio = nextScale / scale;
        float originX = toX - (fromX - left - translationX) * ratio;
        float originY = toY - (fromY - top - translationY) * ratio;
        translationX = boundOrigin(originX, width * nextScale, containerWidth) - left;
        translationY = boundOrigin(originY, height * nextScale, containerHeight) - top;
        scale = nextScale;
    }

    private static float boundOrigin(float origin, float extent, int container) {
        if (extent < container) return (container - extent) * .5f;
        return Math.max(container - extent, Math.min(0f, origin));
    }

    // Moonlight normalizes an absolute input packet against (referenceSize - 1).
    short referenceWidth() { return (short)Math.min(Short.MAX_VALUE, Math.round(width * scale)); }
    short referenceHeight() { return (short)Math.min(Short.MAX_VALUE, Math.round(height * scale)); }
    short remoteX(float x) { return coordinate(x - left - translationX, width * scale, referenceWidth()); }
    short remoteY(float y) { return coordinate(y - top - translationY, height * scale, referenceHeight()); }

    private static short coordinate(float local, float extent, short reference) {
        float normalized = Math.max(0f, Math.min(1f, local / (extent - 1f)));
        return (short)Math.round(normalized * (reference - 1));
    }
}
