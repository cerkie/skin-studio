using System;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Tasks;
using osu.Framework.Allocation;
using osu.Framework.Audio;
using osu.Framework.Audio.Track;
using osu.Framework.Bindables;
using osu.Framework.Audio.Sample;
using osu.Game.Audio;
using osu.Game.Rulesets.Osu.Objects;
using osu.Game.Rulesets.Osu.Skinning.Legacy;
using osu.Game.Rulesets.Scoring;
using osuTK;
using osu.Game.Beatmaps.Legacy;
using osu.Game.Rulesets.Osu.Skinning;
using osu.Framework.Graphics;
using osu.Framework.Graphics.Shapes;
using osu.Framework.Graphics.Containers;
using osu.Game.Graphics;
using osu.Game.Graphics.Sprites;
using osu.Framework.Input.Events;
using osuTK.Input;
using osu.Framework.Graphics.Textures;
using osu.Framework.IO.Stores;
using osu.Framework.Platform;
using osu.Framework.Screens;
using osu.Game;
using osu.Game.Beatmaps;
using osu.Game.Beatmaps.Formats;
using osu.Game.Configuration;
using osu.Game.Database;
using osu.Game.IO;
using osu.Game.Rulesets;
using osu.Game.Rulesets.Mods;
using osu.Game.Rulesets.Objects;
using osu.Game.Rulesets.Objects.Types;
using osu.Game.Rulesets.Osu;
using osu.Game.Rulesets.Osu.Configuration;
using osu.Game.Rulesets.Osu.Mods;
using osu.Game.Screens;
using osu.Game.Screens.Play;
using osu.Game.Screens.Play.HUD;
using osu.Game.Skinning;

namespace LazerPreview
{
    /// <summary>
    /// osu!lazer's gameplay, autoplaying one beatmap, controlled by JSON lines on standard input. Everything on screen is lazer's
    /// own code and assets: the skin, the cursor and its trail, snaking sliders, hit animations and follow points.
    /// </summary>
    public partial class PreviewGame : OsuGameBase
    {
        private OsuScreenStack stack;
        private TransportBar bar;
        private AnimCard animCard;
        private double barStart, barEnd;
        private PreviewPlayer player;

        private string mapPath;
        private double resumeAt;
        private bool wantPlaying = true;
        private double speed = 1;

        private double nextReport;
        private readonly object outLock = new object();

        // ------------------------------------------------------------------------------------------------ protocol out
        private void emit(JsonObject o)
        {
            lock (outLock)
            {
                Console.Out.WriteLine(o.ToJsonString());
                Console.Out.Flush();
            }
        }

        private void emit(string ev, params (string, JsonNode)[] fields)
        {
            var o = new JsonObject { ["event"] = ev };
            foreach (var (k, v) in fields)
                o[k] = v;
            emit(o);
        }

        // ------------------------------------------------------------------------------------------------ start up
        protected override void LoadComplete()
        {
            base.LoadComplete();

            Host.Window.Title = "Skin Studio preview (osu!lazer)";

            // the window is a part of Skin Studio's, so it never has the focus: it must not be throttled as a background window
            Host.MaximumInactiveHz = 1000;

            // a development tool, started over and over by scripts: silent unless asked (SKIN_STUDIO_HOST_VOLUME=0..1)
            double startVolume = double.TryParse(Environment.GetEnvironmentVariable("SKIN_STUDIO_HOST_VOLUME"), System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out double v) ? Math.Clamp(v, 0, 1) : 0;
            Audio.Volume.Value = startVolume;

            // lazer's playfield is 80% of the window's height; here the window is small, so the picture is blown up to fill it
            Add(stack = new OsuScreenStack { RelativeSizeAxes = Axes.Both, Anchor = Anchor.Centre, Origin = Anchor.Centre, Scale = new osuTK.Vector2(1.08f) });
            Add(new InputCatcher(name => emit("key", ("key", name)), () => emit("click")));
            if (Environment.GetEnvironmentVariable("SKIN_STUDIO_NOCARD") == null)
                Add(animCard = new AnimCard());
            if (Environment.GetEnvironmentVariable("SKIN_STUDIO_NOBAR") == null)
            Add(bar = new TransportBar
            {
                PlayPause = () =>
                {
                    wantPlaying = !wantPlaying;
                    applyTransport();
                },
                NewMap = () => emit("newmap"),
                Seek = f => seek(barStart + f * (barEnd - barStart)),
                SetVolume = v =>
                {
                    Audio.Volume.Value = Math.Clamp(v, 0, 1);
                    emit("volume", ("value", Audio.Volume.Value));
                },
                SetSpeed = v =>
                {
                    speed = v;
                    applyTransport();
                    emit("speed", ("value", v));
                },
            });

            // a clean preview: no HUD, a black background, nothing dimming the playfield
            LocalConfig.SetValue(OsuSetting.HUDVisibilityMode, HUDVisibilityMode.Never);
            LocalConfig.SetValue(OsuSetting.DimLevel, 1.0);
            LocalConfig.SetValue(OsuSetting.HitLighting, false);

            Ruleset.Value = new PreviewOsuRuleset().RulesetInfo;
            SelectedMods.Value = new Mod[] { new OsuModAutoplay() };

            // standard input ends when Skin Studio closes, and then so do we
            Task.Run(() =>
            {
                string line;
                while ((line = Console.In.ReadLine()) != null)
                {
                    string l = line;
                    Schedule(() => handle(l));
                }

                Schedule(() => Host.Exit());
            });

            emit("ready");
        }

        // ------------------------------------------------------------------------------------------------ commands in
        private void handle(string line)
        {
            if (string.IsNullOrWhiteSpace(line))
                return;

            JsonNode msg;

            try
            {
                msg = JsonNode.Parse(line);
            }
            catch (JsonException e)
            {
                emit("error", ("message", "bad json: " + e.Message));
                return;
            }

            if (Environment.GetEnvironmentVariable("SKIN_STUDIO_DEBUG") != null)
                emit("debug", ("msg", "cmd " + line));

            try
            {
                switch ((string)msg["cmd"])
                {
                    case "map":
                        mapPath = (string)msg["path"];
                        resumeAt = msg["resume"]?.GetValue<double>() ?? double.NaN;
                        restartPlayer();
                        break;

                    case "skin":
                        applySkin((string)msg["path"], msg["id"]?.GetValue<int>() ?? 0);
                        break;

                    case "play":
                        wantPlaying = true;
                        applyTransport();
                        break;

                    case "pause":
                        wantPlaying = false;
                        applyTransport();
                        break;

                    case "seek":
                        seek(msg["ms"].GetValue<double>());
                        break;

                    case "speed":
                        speed = Math.Clamp(msg["value"].GetValue<double>(), 0.05, 4);
                        applyTransport();
                        break;

                    case "volume":
                        Audio.Volume.Value = Math.Clamp(msg["value"].GetValue<double>(), 0, 1);
                        break;

                    case "followpoint":
                        applyFollowPoint((string)msg["dir"], msg["fps"]?.GetValue<double>() ?? 60, msg["count"]?.GetValue<int>() ?? 0);
                        break;

                    case "embed":
                        embedParent = new IntPtr(msg["parent"].GetValue<long>());

                        if (Environment.GetEnvironmentVariable("SKIN_STUDIO_EMBED_LATE") == null)
                            doEmbed();

                        break;

                    case "bounds":
                        if (msg["scale"] != null && stack != null && msg["scale"].GetValue<float>() > 0.1f)
                            stack.Scale = new osuTK.Vector2(msg["scale"].GetValue<float>());

                        if (embedChild != IntPtr.Zero && embedParent != IntPtr.Zero)
                            Embedding.Place(embedChild, msg["x"].GetValue<int>(), msg["y"].GetValue<int>(), msg["w"].GetValue<int>(), msg["h"].GetValue<int>(), msg["visible"]?.GetValue<bool>() ?? true);

                        break;

                    case "cursor":
                    {
                        var cc = osu.Framework.Testing.TestingExtensions.ChildrenOfType<osu.Game.Rulesets.Osu.UI.Cursor.OsuCursorContainer>(player).FirstOrDefault();
                        var ac = cc == null ? null : osu.Framework.Testing.TestingExtensions.ChildrenOfType<osu.Game.Rulesets.Osu.UI.Cursor.OsuCursor>(cc).FirstOrDefault();
                        var sc = cc == null ? null : osu.Framework.Testing.TestingExtensions.ChildrenOfType<osu.Game.Rulesets.Osu.Skinning.Legacy.LegacyCursor>(cc).FirstOrDefault();
                        emit("debug", ("msg", "cursor container=" + (cc != null) + " active=" + (ac != null) + " legacy=" + (sc != null) + " legacyExpand=" + (sc?.ExpandTarget?.Scale.ToString() ?? "n/a") + " legacyRot=" + (sc?.ExpandTarget?.Rotation.ToString() ?? "n/a") + " expand=" + (ac?.CurrentExpandedScale.ToString() ?? "n/a") + " rot=" + (ac?.CurrentRotation.ToString() ?? "n/a") + " time=" + clock?.CurrentTime));
                        break;
                    }

                    case "slider":
                    {
                        double want = msg["start"].GetValue<double>();
                        var ds = osu.Framework.Testing.TestingExtensions.ChildrenOfType<osu.Game.Rulesets.Osu.Objects.Drawables.DrawableSlider>(player).FirstOrDefault(s => Math.Abs(s.HitObject.StartTime - want) < 1);

                        if (ds == null)
                        {
                            emit("debug", ("msg", "slider none time=" + clock?.CurrentTime));
                            break;
                        }

                        var body = ds.Body;
                        var head = ds.HeadCircle;
                        var tail = ds.TailCircle;
                        var ball = ds.Ball;
                        var fc = osu.Framework.Testing.TestingExtensions.ChildrenOfType<osu.Game.Rulesets.Osu.Skinning.FollowCircle>(ds).FirstOrDefault();
                        var snake = osu.Framework.Testing.TestingExtensions.ChildrenOfType<osu.Game.Rulesets.Osu.Skinning.SnakingSliderBody>(ds).FirstOrDefault();
                        emit("debug", ("msg", "slider time=" + clock?.CurrentTime + " alive=" + ds.IsAlive + " present=" + ds.IsPresent + " alpha=" + ds.Alpha + " body=" + body.Alpha + " head=" + head.Alpha + " tail=" + tail.Alpha + " headPiece=" + head.CirclePiece.Alpha + "/" + head.CirclePiece.Scale.X + " headScale=" + head.Scale.X + " tailPiece=" + tail.CirclePiece.Alpha + " headDrawAlpha=" + head.DrawColourInfo.Colour.TopLeft.Linear.A + " ball=" + ball.Alpha + " ballScale=" + ball.Scale.X + " fc=" + (fc == null ? "n/a" : fc.Alpha + "/" + fc.Scale.X) + " snake=" + (snake == null ? "n/a" : snake.SnakedStart + "|" + snake.SnakedEnd) + " state=" + ds.State.Value));
                        break;
                    }

                    case "headtree":
                    {
                        double want = msg["start"].GetValue<double>();
                        var ds = osu.Framework.Testing.TestingExtensions.ChildrenOfType<osu.Game.Rulesets.Osu.Objects.Drawables.DrawableSlider>(player).FirstOrDefault(s => Math.Abs(s.HitObject.StartTime - want) < 1);

                        if (ds == null)
                        {
                            emit("debug", ("msg", "headtree none"));
                            break;
                        }

                        var sb = new System.Text.StringBuilder("headtree time=" + clock?.CurrentTime + " |");

                        sb.Append(" head=" + ds.HeadCircle.Alpha.ToString("0.##"));

                        foreach (var d in osu.Framework.Testing.TestingExtensions.ChildrenOfType<Drawable>(msg["part"]?.GetValue<string>() == "ball" ? ds.Ball : ds.HeadCircle))
                            sb.Append(' ').Append(d.GetType().Name).Append('=').Append(d.Alpha.ToString("0.##")).Append('/').Append(d.DrawColourInfo.Colour.TopLeft.Linear.A.ToString("0.##")).Append('/').Append(d.Scale.X.ToString("0.##"));

                        emit("debug", ("msg", sb.ToString()));
                        break;
                    }

                    case "probe":
                    {
                        var sk = SkinManager.CurrentSkin.Value;
                        emit("debug", ("msg", "skin " + sk.SkinInfo.Value.Name + " cursortrail=" + (sk.GetTexture("cursortrail")?.Width.ToString() ?? "null") + " cursor=" + (sk.GetTexture("cursor")?.Width.ToString() ?? "null") + " cursormiddle=" + (sk.GetTexture("cursormiddle")?.Width.ToString() ?? "null")));
                        break;
                    }

                    case "offset":
                        LocalConfig.SetValue(OsuSetting.AudioOffset, Math.Clamp(msg["ms"].GetValue<double>(), -500, 500));
                        break;

                    case "config":
                        applyConfig(msg);
                        break;

                    case "quit":
                        Host.Exit();
                        break;

                    default:
                        emit("error", ("message", "unknown command " + (string)msg["cmd"]));
                        break;
                }
            }
            catch (Exception e)
            {
                emit("error", ("message", e.GetType().Name + ": " + e.Message));
            }
        }

        private IntPtr embedParent, embedChild;
        private JsonNode lastBounds;
        private bool embedded;

        private void doEmbed()
        {
            embedChild = nativeHandle("WindowHandle");

            if (embedChild != IntPtr.Zero && embedParent != IntPtr.Zero)
                Embedding.Attach(embedChild, embedParent);
            else
                emit("error", ("message", "the window could not be embedded"));
        }

        private IntPtr nativeHandle(string property)
        {
            var p = Host.Window?.GetType().GetProperty(property);
            return p?.GetValue(Host.Window) is IntPtr h ? h : IntPtr.Zero;
        }

        private void applyConfig(JsonNode msg)
        {
            if (msg["hitLighting"] != null)
                LocalConfig.SetValue(OsuSetting.HitLighting, msg["hitLighting"].GetValue<bool>());

            if (msg["cursorSize"] != null)
                LocalConfig.SetValue(OsuSetting.GameplayCursorSize, msg["cursorSize"].GetValue<float>());

            OsuRulesetConfigManager cfg;

            try
            {
                cfg = Dependencies.Get<IRulesetConfigCache>().GetConfigFor(new OsuRuleset()) as OsuRulesetConfigManager;
            }
            catch (InvalidOperationException)
            {
                // the game is still loading its settings: try again shortly
                Scheduler.AddDelayed(() => applyConfig(msg), 500);
                return;
            }

            if (cfg != null)
            {
                if (msg["snakingIn"] != null)
                    cfg.SetValue(OsuRulesetSetting.SnakingInSliders, msg["snakingIn"].GetValue<bool>());
                if (msg["snakingOut"] != null)
                    cfg.SetValue(OsuRulesetSetting.SnakingOutSliders, msg["snakingOut"].GetValue<bool>());
                if (msg["hitAnimations"] != null)
                    cfg.SetValue(OsuRulesetSetting.HitAnimations, msg["hitAnimations"].GetValue<bool>());
                if (msg["cursorTrail"] != null)
                    cfg.SetValue(OsuRulesetSetting.ShowCursorTrail, msg["cursorTrail"].GetValue<bool>());
            }
        }

        // ------------------------------------------------------------------------------------------------ skin
        private async void applySkin(string path, int id)
        {
            try
            {
                if (path == "argonpro")
                {
                    Schedule(() =>
                    {
                        var rl = Dependencies.Get<RealmAccess>();
                        var live = rl.Run(r => r.Find<SkinInfo>(new Guid("9FC9CF5D-0F16-4C71-8256-98868321AC43"))?.ToLive(rl));

                        if (live != null)
                            SkinManager.CurrentSkinInfo.Value = live;

                        emit("skin", ("id", id), ("ok", live != null));
                    });
                    return;
                }

                if (string.IsNullOrEmpty(path))
                {
                    Schedule(() =>
                    {
                        SkinManager.CurrentSkinInfo.SetDefault();
                        emit("skin", ("id", id), ("ok", true));
                    });
                    return;
                }

                var imported = await SkinManager.Import(new ImportTask(path)).ConfigureAwait(false);

                Schedule(() =>
                {
                    if (imported != null)
                        SkinManager.CurrentSkinInfo.Value = imported;

                    emit("skin", ("id", id), ("ok", imported != null));
                });
            }
            catch (Exception e)
            {
                emit("skin", ("id", id), ("ok", false), ("message", e.Message));
            }
        }

        // ------------------------------------------------------------------------------------------------ follow points
        private void applyFollowPoint(string dir, double fps, int count)
        {
            var frames = new System.Collections.Generic.List<Texture>();

            for (int i = 0; i < count; i++)
            {
                string f = Path.Combine(dir, "followpoint-" + i + ".png");

                if (!File.Exists(f))
                    break;

                using var s = File.OpenRead(f);
                var tex = Texture.FromStream(Host.Renderer, s);

                if (tex == null)
                    break;

                frames.Add(tex);
            }

            FrameSkin.Frames = frames.ToArray();
            FrameSkin.FrameLength = 1000.0 / Math.Max(1, fps);

            // the transformer is built when the playfield is, so the playfield is built again, where it was
            if (player != null)
                pendingRestart = true;
        }

        // ------------------------------------------------------------------------------------------------ the player
        private void restartPlayer()
        {
            started = false;
            pendingRestart = false;

            if (player != null)
            {
                var old = player;
                player = null;

                // the old map's music must not outlive it
                Beatmap.Value?.Track?.Stop();
                old.Exit();
                Scheduler.AddDelayed(() => emit("debug", ("msg", "current screen " + stack.CurrentScreen?.GetType().Name + " running=" + Beatmap.Value?.Track?.IsRunning)), 250);
                Scheduler.AddDelayed(restartPlayer, 300);
                return;
            }

            // the ruleset settings load in the background at start up; the playfield cannot be made before they have
            try
            {
                Dependencies.Get<IRulesetConfigCache>().GetConfigFor(new PreviewOsuRuleset());
            }
            catch (InvalidOperationException)
            {
                Scheduler.AddDelayed(restartPlayer, 300);
                return;
            }

            if (string.IsNullOrEmpty(mapPath) || !File.Exists(mapPath))
            {
                emit("error", ("message", "map not found: " + mapPath));
                return;
            }

            var working = new FileWorkingBeatmap(mapPath, Audio);
            working.BeatmapInfo.Ruleset = new PreviewOsuRuleset().RulesetInfo;
            var previous = Beatmap.Value;
            Beatmap.Value = working;
            working.LoadTrack();
            previous?.Track?.Stop();

            var p = new PreviewPlayer();
            player = p;
            stack.Push(p);

            emit("loaded", ("title", working.BeatmapInfo.Metadata.Title), ("version", working.BeatmapInfo.DifficultyName));
        }

        private MasterGameplayClockContainer clock => player?.Master;

        private bool started;
        private bool pendingRestart;
        private double dbgNext;

        private void applyTransport()
        {
            var c = clock;

            if (c == null || !c.IsLoaded)
                return;

            c.UserPlaybackRate.Value = speed;

            if (wantPlaying && c.IsPaused.Value)
                c.Start();
            else if (!wantPlaying && !c.IsPaused.Value)
                c.Stop();
        }

        private void seek(double ms)
        {
            var c = clock;

            if (c == null || !c.IsLoaded)
                return;

            c.Seek(ms);
            applyTransport();
        }

        protected override void Update()
        {
            base.Update();

            var c = clock;

            if (Environment.GetEnvironmentVariable("SKIN_STUDIO_DEBUG") != null && Clock.CurrentTime > dbgNext)
            {
                dbgNext = Clock.CurrentTime + 2000;
                emit("debug", ("msg", "card " + (animCard == null ? "none" : animCard.Position + " size " + animCard.DrawSize + " alpha " + animCard.Alpha + " window " + DrawSize)));
                emit("debug", ("msg", "player=" + (player != null) + " loaded=" + (player?.IsLoaded) + " clock=" + (c != null) + " clockLoaded=" + (c?.IsLoaded) + " pending=" + pendingRestart + " started=" + started));
            }

            if (c == null || !c.IsLoaded || player?.IsLoaded != true)
                return;

            if (pendingRestart && started)
            {
                pendingRestart = false;
                resumeAt = c.CurrentTime;
                restartPlayer();
                return;
            }

            if (!started)
            {
                started = true;

                if (!embedded && embedParent != IntPtr.Zero && Environment.GetEnvironmentVariable("SKIN_STUDIO_EMBED_LATE") != null)
                {
                    embedded = true;
                    doEmbed();
                    emit("debug", ("msg", "embedded late"));
                }

                if (!double.IsNaN(resumeAt))
                    c.Seek(resumeAt);

                applyTransport();
            }

            var hits = player.GameplayState?.Beatmap?.HitObjects;
            double end = hits != null && hits.Count > 0 ? hits.Max(h => h.GetEndTime()) : 0;
            double first = hits != null && hits.Count > 0 ? hits[0].StartTime : 0;
            double start = Math.Min(c.StartTime, first - 2500);

            // loop: when the map has finished, play it again
            if (end > 0 && c.CurrentTime > end + 1500)
                c.Seek(start);

            barStart = start;
            barEnd = end + 1500;
            bar?.Refresh(!c.IsPaused.Value, barEnd > barStart ? (c.CurrentTime - barStart) / (barEnd - barStart) : 0, fmtTime(c.CurrentTime - barStart) + " / " + fmtTime(barEnd - barStart), Audio.Volume.Value, speed);

            if (Clock.CurrentTime >= nextReport)
            {
                nextReport = Clock.CurrentTime + 100;
                emit("time", ("ms", c.CurrentTime), ("start", start), ("end", end + 1500), ("playing", !c.IsPaused.Value), ("fps", Clock.FramesPerSecond));
            }
        }

        private static string fmtTime(double ms)
        {
            int s = (int)(Math.Max(ms, 0) / 1000);
            return (s / 60) + ":" + (s % 60).ToString("00");
        }

        protected override void Dispose(bool isDisposing)
        {
            base.Dispose(isDisposing);
        }
    }

    /// <summary>Tells Skin Studio about clicks and keys on the preview, which can have the keyboard focus when it is embedded.</summary>
    public partial class InputCatcher : Box
    {
        private readonly Action<string> key;
        private readonly Action click;

        public InputCatcher(Action<string> key, Action click)
        {
            this.key = key;
            this.click = click;
            RelativeSizeAxes = Axes.Both;
            Alpha = 0;
        }

        public override bool IsPresent => true;

        protected override bool OnClick(ClickEvent e)
        {
            click();
            return true;
        }

        protected override bool OnKeyDown(KeyDownEvent e)
        {
            if (e.Repeat)
                return false;

            string name = e.Key switch
            {
                Key.Space => "space",
                Key.Left => "left",
                Key.Right => "right",
                Key.Up => "up",
                Key.Down => "down",
                _ => null,
            };

            if (name == null)
                return false;

            key(name);
            return true;
        }
    }

    /// <summary>The animation Skin Studio is previewing, as a skin that has only follow point frames.</summary>
    public class FrameSkin : ISkin
    {
        public static Texture[] Frames = Array.Empty<Texture>();
        public static double FrameLength = 1000.0 / 60;

        public Drawable GetDrawableComponent(ISkinComponentLookup lookup) => null;

        public Texture GetTexture(string componentName, WrapMode wrapModeS, WrapMode wrapModeT)
        {
            const string prefix = "followpoint-";

            if (componentName.StartsWith(prefix, StringComparison.Ordinal) && int.TryParse(componentName.Substring(prefix.Length), out int i) && i >= 0 && i < Frames.Length)
                return Frames[i];

            return null;
        }

        public ISample GetSample(ISampleInfo sampleInfo) => null;

        public IBindable<TValue> GetConfig<TLookup, TValue>(TLookup lookup) => null;
    }

    /// <summary>lazer's own skin transformers, with the follow point replaced by the animation being made.</summary>
    public class PreviewOsuRuleset : OsuRuleset
    {
        public override ISkin CreateSkinTransformer(ISkin skin, IBeatmap beatmap)
        {
            var inner = base.CreateSkinTransformer(skin, beatmap);

            return inner == null ? null : new FollowPointTransformer(inner);
        }

        private class FollowPointTransformer : SkinTransformer
        {
            private readonly FrameSkin frames = new FrameSkin();

            public FollowPointTransformer(ISkin skin)
                : base(skin)
            {
            }

            public override Drawable GetDrawableComponent(ISkinComponentLookup lookup)
            {
                if (lookup is OsuSkinComponentLookup l && l.Component == OsuSkinComponents.FollowPoint && FrameSkin.Frames.Length > 0)
                {
                    return frames.GetAnimation("followpoint", true, true, false, startAtCurrentTime: false, frameLength: FrameSkin.FrameLength,
                        maxSize: new Vector2(OsuHitObject.OBJECT_RADIUS * 2, OsuHitObject.OBJECT_RADIUS));
                }

                return base.GetDrawableComponent(lookup);
            }
        }
    }

    /// <summary>Autoplay with everything that is not the playfield switched off.</summary>
    public partial class PreviewPlayer : ReplayPlayer
    {
        public PreviewPlayer()
            : base((beatmap, mods) => mods.OfType<ModAutoplay>().First().CreateScoreFromReplayData(beatmap, mods), new PlayerConfiguration
            {
                ShowResults = false,
                AllowPause = false,
                AllowRestart = false,
                AllowSkipping = false,
                AutomaticallySkipIntro = true,
                ShowFailingOverlay = false,
                AllowUserInteraction = false,
            })
        {
        }

        public MasterGameplayClockContainer Master => GameplayClockContainer as MasterGameplayClockContainer;

        protected override void LoadComplete()
        {
            base.LoadComplete();

            // the "Watching autoplay" overlay and its settings button
            if (ReplayOverlay != null)
                ReplayOverlay.Alpha = 0;
        }
    }

    /// <summary>A working beatmap read from a .osu file, with its audio from the same folder.</summary>
    public class FileWorkingBeatmap : WorkingBeatmap
    {
        private readonly Beatmap beatmap;
        private readonly string dir;
        private readonly AudioManager audioManager;

        public FileWorkingBeatmap(string path, AudioManager audio)
            : this(read(path), audio)
        {
            dir = Path.GetDirectoryName(Path.GetFullPath(path));
        }

        private FileWorkingBeatmap(Beatmap beatmap, AudioManager audio)
            : base(beatmap.BeatmapInfo, audio)
        {
            this.beatmap = beatmap;
            audioManager = audio;
        }

        private static Beatmap read(string path)
        {
            using var stream = File.OpenRead(path);
            using var reader = new LineBufferedReader(stream);
            return Decoder.GetDecoder<Beatmap>(reader).Decode(reader);
        }

        protected override IBeatmap GetBeatmap() => beatmap;

        public override Texture GetBackground() => null;

        protected override Track GetBeatmapTrack()
        {
            string audio = beatmap.Metadata.AudioFile;

            if (!string.IsNullOrEmpty(audio) && dir != null && File.Exists(Path.Combine(dir, audio)))
            {
                var store = audioManager.GetTrackStore(new StorageBackedResourceStore(new NativeStorage(dir)));
                return store.Get(audio);
            }

            // no audio file: a silent track as long as the map
            return new TrackVirtual(beatmap.HitObjects.Count == 0 ? 10000 : beatmap.HitObjects.Max(h => h.GetEndTime()) + 5000);
        }

        protected override ISkin GetSkin() => null;

        public override Stream GetStream(string storagePath) => null;
    }
}
