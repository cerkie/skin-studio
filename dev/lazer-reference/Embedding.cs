using System;
using System.Runtime.InteropServices;

namespace LazerPreview
{
    /// <summary>
    /// Puts the preview's window inside a window of Skin Studio (a child window on Windows, a reparented X11 window on Linux),
    /// and keeps it in a rectangle of it.
    /// </summary>
    internal static class Embedding
    {
        // ---- Windows
        private const int GWL_STYLE = -16;
        private const long WS_POPUP = 0x80000000, WS_CAPTION = 0xC00000, WS_THICKFRAME = 0x40000, WS_SYSMENU = 0x80000, WS_MINIMIZEBOX = 0x20000, WS_MAXIMIZEBOX = 0x10000;
        private const long WS_CHILD = 0x40000000, WS_VISIBLE = 0x10000000, WS_CLIPSIBLINGS = 0x04000000;
        private const uint SWP_NOZORDER = 0x4, SWP_NOACTIVATE = 0x10, SWP_FRAMECHANGED = 0x20, SWP_SHOWWINDOW = 0x40, SWP_HIDEWINDOW = 0x80;

        [DllImport("user32.dll")] private static extern IntPtr SetParent(IntPtr child, IntPtr parent);
        [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] private static extern IntPtr GetWindowLongPtr(IntPtr h, int index);
        [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")] private static extern IntPtr SetWindowLongPtr(IntPtr h, int index, IntPtr value);
        [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);

        // ---- X11
        [DllImport("libX11.so.6")] private static extern IntPtr XOpenDisplay(IntPtr name);
        [DllImport("libX11.so.6")] private static extern int XReparentWindow(IntPtr d, ulong w, ulong parent, int x, int y);
        [DllImport("libX11.so.6")] private static extern int XMoveResizeWindow(IntPtr d, ulong w, int x, int y, uint width, uint height);
        [DllImport("libX11.so.6")] private static extern int XMapWindow(IntPtr d, ulong w);
        [DllImport("libX11.so.6")] private static extern int XUnmapWindow(IntPtr d, ulong w);
        [DllImport("libX11.so.6")] private static extern int XFlush(IntPtr d);

        private static IntPtr display;

        public static bool Supported => RuntimeInformation.IsOSPlatform(OSPlatform.Windows) || RuntimeInformation.IsOSPlatform(OSPlatform.Linux);

        public static void Attach(IntPtr child, IntPtr parent)
        {
            if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            {
                SetParent(child, parent);
                long style = GetWindowLongPtr(child, GWL_STYLE).ToInt64();
                style &= ~(WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX);
                style |= WS_CHILD | WS_CLIPSIBLINGS;
                SetWindowLongPtr(child, GWL_STYLE, new IntPtr(style));
                SetWindowPos(child, IntPtr.Zero, 0, 0, 16, 16, SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW);
            }
            else
            {
                display = XOpenDisplay(IntPtr.Zero);
                XUnmapWindow(display, (ulong)child);
                XReparentWindow(display, (ulong)child, (ulong)parent, 0, 0);
                XFlush(display);
            }
        }

        /// <summary>Position in physical pixels, relative to the parent's client area.</summary>
        public static void Place(IntPtr child, int x, int y, int w, int h, bool visible)
        {
            w = Math.Max(w, 1);
            h = Math.Max(h, 1);

            if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            {
                SetWindowPos(child, IntPtr.Zero, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | (visible ? SWP_SHOWWINDOW : SWP_HIDEWINDOW));
            }
            else if (display != IntPtr.Zero)
            {
                XMoveResizeWindow(display, (ulong)child, x, y, (uint)w, (uint)h);
                if (visible)
                    XMapWindow(display, (ulong)child);
                else
                    XUnmapWindow(display, (ulong)child);
                XFlush(display);
            }
        }
    }
}
