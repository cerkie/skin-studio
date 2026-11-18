using osu.Framework;
using osu.Framework.Platform;

namespace LazerPreview
{
    /// <summary>
    /// The gameplay preview of Skin Studio, drawn by osu!lazer's own renderer. Skin Studio starts this program and talks to it
    /// with one JSON object per line on standard input; it answers with one JSON object per line on standard output.
    /// </summary>
    public static class Program
    {
        public static void Main(string[] args)
        {
            // standard output carries the protocol only; the framework's own logging goes to its log files
            using GameHost host = Host.GetSuitableDesktopHost("skin-studio-lazer-preview");
            using var game = new PreviewGame();
            host.Run(game);
        }
    }
}
