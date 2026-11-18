using System;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Tasks;
using osu.Framework.Allocation;
using osu.Framework.Audio;
using osu.Framework.Audio.Track;
using osu.Framework.Graphics;
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

            Add(stack = new OsuScreenStack { RelativeSizeAxes = Axes.Both });

            // a clean preview: no HUD, a black background, nothing dimming the playfield
            LocalConfig.SetValue(OsuSetting.HUDVisibilityMode, HUDVisibilityMode.Never);
            LocalConfig.SetValue(OsuSetting.DimLevel, 1.0);
            LocalConfig.SetValue(OsuSetting.HitLighting, false);

            Ruleset.Value = new OsuRuleset().RulesetInfo;
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

        private void applyConfig(JsonNode msg)
        {
            if (msg["hitLighting"] != null)
                LocalConfig.SetValue(OsuSetting.HitLighting, msg["hitLighting"].GetValue<bool>());

            if (msg["cursorSize"] != null)
                LocalConfig.SetValue(OsuSetting.GameplayCursorSize, msg["cursorSize"].GetValue<float>());

            var cfg = Dependencies.Get<IRulesetConfigCache>().GetConfigFor(new OsuRuleset()) as OsuRulesetConfigManager;

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

        // ------------------------------------------------------------------------------------------------ the player
        private void restartPlayer()
        {
            if (player != null)
            {
                var old = player;
                player = null;
                old.Exit();
                Scheduler.AddDelayed(restartPlayer, 300);
                return;
            }

            if (string.IsNullOrEmpty(mapPath) || !File.Exists(mapPath))
            {
                emit("error", ("message", "map not found: " + mapPath));
                return;
            }

            var working = new FileWorkingBeatmap(mapPath, Audio);
            Beatmap.Value = working;
            working.LoadTrack();

            var p = new PreviewPlayer();
            player = p;
            stack.Push(p);

            emit("loaded", ("title", working.BeatmapInfo.Metadata.Title), ("version", working.BeatmapInfo.DifficultyName));
        }

        private MasterGameplayClockContainer clock => player?.Master;

        private bool started;

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

            if (c == null || !c.IsLoaded || player?.IsLoaded != true)
                return;

            if (!started)
            {
                started = true;

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

            if (Clock.CurrentTime >= nextReport)
            {
                nextReport = Clock.CurrentTime + 100;
                emit("time", ("ms", c.CurrentTime), ("start", start), ("end", end + 1500), ("playing", !c.IsPaused.Value));
            }
        }

        protected override void Dispose(bool isDisposing)
        {
            base.Dispose(isDisposing);
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
