using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.WinForms;
using Microsoft.Win32;

namespace HeiGate.Desktop;

internal static class Program
{
    private const string MutexName = @"Local\HeiGate.Desktop.SingleInstance";
    private const string ShowEventName = @"Local\HeiGate.Desktop.ShowWindow";

    [STAThread]
    private static void Main()
    {
        using var showEvent = new EventWaitHandle(false, EventResetMode.AutoReset, ShowEventName);
        using var mutex = new Mutex(true, MutexName, out var isFirstInstance);
        if (!isFirstInstance)
        {
            showEvent.Set();
            return;
        }

        ApplicationConfiguration.Initialize();
        using var context = new HeiGateApplicationContext();
        var registration = ThreadPool.RegisterWaitForSingleObject(
            showEvent,
            (_, _) => context.ShowWindow(),
            null,
            Timeout.Infinite,
            executeOnlyOnce: false);

        Application.Run(context);
        registration.Unregister(null);
        mutex.ReleaseMutex();
    }
}

internal sealed class HeiGateApplicationContext : ApplicationContext
{
    private readonly MainWindow _window;
    private readonly NotifyIcon _trayIcon;
    private readonly ToolStripMenuItem _autoStartItem;
    private bool _exiting;

    public HeiGateApplicationContext()
    {
        _window = new MainWindow();
        MainForm = _window;
        _window.FormClosing += OnWindowClosing;

        _autoStartItem = new ToolStripMenuItem("开机自动启动")
        {
            Checked = AutoStart.IsEnabled,
            CheckOnClick = false,
        };
        _autoStartItem.Click += (_, _) => ToggleAutoStart();

        var menu = new ContextMenuStrip();
        menu.Items.Add("显示窗口", null, (_, _) => ShowWindow());
        menu.Items.Add("重启本地网关", null, async (_, _) => await _window.RestartGatewayAsync());
        menu.Items.Add(_autoStartItem);
        menu.Items.Add(new ToolStripSeparator());
        menu.Items.Add("退出 HeiGate", null, (_, _) => ExitApplication());

        // ExtractAssociatedIcon only yields the 32px image, which the tray then
        // shrinks; ask for the small size so the hand-drawn 16px image is used.
        var icon = Icon.ExtractIcon(Application.ExecutablePath, 0, SystemInformation.SmallIconSize.Width)
            ?? Icon.ExtractAssociatedIcon(Application.ExecutablePath)
            ?? SystemIcons.Application;
        _trayIcon = new NotifyIcon
        {
            Icon = icon,
            Text = "HeiGate 本地网关",
            ContextMenuStrip = menu,
            Visible = true,
        };
        _trayIcon.MouseClick += (_, eventArgs) =>
        {
            if (eventArgs.Button == MouseButtons.Left)
            {
                ShowWindow();
            }
        };

        _window.Show();
    }

    public void ShowWindow()
    {
        if (_exiting || _window.IsDisposed)
        {
            return;
        }

        if (_window.InvokeRequired)
        {
            try
            {
                _window.BeginInvoke(new Action(ShowWindow));
            }
            catch (InvalidOperationException)
            {
                // The app is already shutting down.
            }
            return;
        }

        if (!_window.Visible)
        {
            _window.Show();
        }
        if (_window.WindowState == FormWindowState.Minimized)
        {
            _window.WindowState = FormWindowState.Normal;
        }
        _window.Activate();
        _window.BringToFront();
    }

    private void OnWindowClosing(object? sender, FormClosingEventArgs eventArgs)
    {
        if (_exiting)
        {
            return;
        }

        eventArgs.Cancel = true;
        _window.Hide();
    }

    private void ToggleAutoStart()
    {
        try
        {
            AutoStart.SetEnabled(!AutoStart.IsEnabled);
            _autoStartItem.Checked = AutoStart.IsEnabled;
        }
        catch (Exception exception)
        {
            MessageBox.Show(
                $"无法更新开机启动设置：{exception.Message}",
                "HeiGate",
                MessageBoxButtons.OK,
                MessageBoxIcon.Warning);
        }
    }

    private void ExitApplication()
    {
        if (_exiting)
        {
            return;
        }

        _exiting = true;
        _trayIcon.Visible = false;
        _window.Shutdown();
        _window.Close();
        ExitThread();
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            _trayIcon.Dispose();
            _window.Dispose();
        }
        base.Dispose(disposing);
    }
}

internal sealed class MainWindow : Form
{
    private const string WebViewRuntimeUrl = "https://developer.microsoft.com/microsoft-edge/webview2/";

    private readonly WebView2 _webView;
    private readonly Label _statusLabel;
    private readonly GatewayProcess _gateway = new();
    private readonly SemaphoreSlim _restartLock = new(1, 1);
    private bool _started;
    private bool _shuttingDown;

    public MainWindow()
    {
        Text = "HeiGate";
        StartPosition = FormStartPosition.CenterScreen;
        Size = new Size(1440, 920);
        MinimumSize = new Size(1000, 680);
        BackColor = Color.FromArgb(246, 247, 249);
        Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath);

        _statusLabel = new Label
        {
            Dock = DockStyle.Fill,
            Text = "正在启动本地网关…",
            TextAlign = ContentAlignment.MiddleCenter,
            Font = new Font("Microsoft YaHei UI", 11F, FontStyle.Regular),
            ForeColor = Color.FromArgb(71, 85, 105),
        };

        _webView = new WebView2
        {
            Dock = DockStyle.Fill,
            Visible = false,
        };

        Controls.Add(_webView);
        Controls.Add(_statusLabel);
        Shown += async (_, _) =>
        {
            if (_started)
            {
                return;
            }
            _started = true;
            await StartGatewayAndOpenAsync();
        };

        _gateway.UnexpectedExit += (_, _) =>
        {
            if (_shuttingDown || IsDisposed)
            {
                return;
            }
            try
            {
                BeginInvoke(new Action(async () =>
                {
                    await Task.Delay(750);
                    await StartGatewayAndOpenAsync();
                }));
            }
            catch (InvalidOperationException)
            {
                // The form was disposed while the child process was exiting.
            }
        };
    }

    public async Task RestartGatewayAsync()
    {
        if (_shuttingDown)
        {
            return;
        }
        _gateway.Stop();
        await StartGatewayAndOpenAsync();
    }

    public void Shutdown()
    {
        _shuttingDown = true;
        _gateway.Dispose();
        _webView.Dispose();
    }

    private async Task StartGatewayAndOpenAsync()
    {
        await _restartLock.WaitAsync();
        try
        {
            if (_shuttingDown)
            {
                return;
            }

            _statusLabel.Text = "正在启动本地网关…";
            _statusLabel.Visible = true;
            _webView.Visible = false;

            var baseUrl = await _gateway.StartAsync();
            if (_webView.CoreWebView2 is null)
            {
                var webViewData = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                    "HeiGate",
                    "WebView2");
                Directory.CreateDirectory(webViewData);
                var environment = await CoreWebView2Environment.CreateAsync(null, webViewData);
                await _webView.EnsureCoreWebView2Async(environment);
                var coreWebView = _webView.CoreWebView2
                    ?? throw new InvalidOperationException("WebView2 初始化失败。");
                coreWebView.Settings.AreDevToolsEnabled = false;
                coreWebView.Settings.IsStatusBarEnabled = false;
                coreWebView.NewWindowRequested += (_, eventArgs) =>
                {
                    eventArgs.Handled = true;
                    OpenExternal(eventArgs.Uri);
                };
            }

            _webView.Source = new Uri(baseUrl);
            _webView.Visible = true;
            _statusLabel.Visible = false;
        }
        catch (WebView2RuntimeNotFoundException)
        {
            _statusLabel.Text = "缺少 Microsoft Edge WebView2 Runtime";
            var result = MessageBox.Show(
                "HeiGate 需要 Microsoft Edge WebView2 Runtime。是否打开微软官方下载页？",
                "HeiGate",
                MessageBoxButtons.YesNo,
                MessageBoxIcon.Warning);
            if (result == DialogResult.Yes)
            {
                OpenExternal(WebViewRuntimeUrl);
            }
        }
        catch (Exception exception)
        {
            _statusLabel.Text = "本地网关启动失败";
            MessageBox.Show(
                exception.Message,
                "HeiGate",
                MessageBoxButtons.OK,
                MessageBoxIcon.Error);
        }
        finally
        {
            _restartLock.Release();
        }
    }

    private static void OpenExternal(string url)
    {
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri) ||
            (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
        {
            return;
        }
        Process.Start(new ProcessStartInfo(uri.AbsoluteUri) { UseShellExecute = true });
    }
}

internal sealed class GatewayProcess : IDisposable
{
    private static readonly HttpClient HttpClient = new() { Timeout = TimeSpan.FromSeconds(1) };

    private readonly object _sync = new();
    private Process? _process;
    private bool _stopping;
    private string _baseUrl = string.Empty;

    public event EventHandler? UnexpectedExit;

    public async Task<string> StartAsync()
    {
        lock (_sync)
        {
            if (_process is { HasExited: false })
            {
                return _baseUrl;
            }
        }

        var executable = Path.Combine(AppContext.BaseDirectory, "heigate-gateway.exe");
        if (!File.Exists(executable))
        {
            throw new FileNotFoundException("找不到本地网关程序 heigate-gateway.exe。", executable);
        }

        var port = FindAvailablePort();
        var dataDirectory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "HeiGate");
        Directory.CreateDirectory(dataDirectory);
        var logDirectory = Path.Combine(dataDirectory, "logs");
        Directory.CreateDirectory(logDirectory);
        var logPath = Path.Combine(logDirectory, "desktop.log");

        var startInfo = new ProcessStartInfo
        {
            FileName = executable,
            WorkingDirectory = AppContext.BaseDirectory,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        startInfo.Environment["HEIGATE_HOST"] = "127.0.0.1";
        startInfo.Environment["HEIGATE_PORT"] = port.ToString();
        startInfo.Environment["DATA_DIR"] = dataDirectory;

        var process = new Process
        {
            StartInfo = startInfo,
            EnableRaisingEvents = true,
        };
        process.OutputDataReceived += (_, eventArgs) => AppendLog(logPath, eventArgs.Data);
        process.ErrorDataReceived += (_, eventArgs) => AppendLog(logPath, eventArgs.Data);
        process.Exited += (_, _) =>
        {
            var unexpected = false;
            lock (_sync)
            {
                if (ReferenceEquals(_process, process))
                {
                    _process = null;
                    unexpected = !_stopping;
                }
            }
            if (unexpected)
            {
                UnexpectedExit?.Invoke(this, EventArgs.Empty);
            }
        };

        lock (_sync)
        {
            _stopping = false;
            _baseUrl = $"http://127.0.0.1:{port}";
            _process = process;
        }

        try
        {
            if (!process.Start())
            {
                throw new InvalidOperationException("本地网关进程未能启动。");
            }
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();
            await WaitUntilHealthyAsync(process, _baseUrl);
            return _baseUrl;
        }
        catch
        {
            Stop();
            throw;
        }
    }

    public void Stop()
    {
        Process? process;
        lock (_sync)
        {
            _stopping = true;
            process = _process;
            _process = null;
        }

        if (process is null)
        {
            return;
        }
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
                process.WaitForExit(2_000);
            }
        }
        catch (Exception exception) when (exception is InvalidOperationException or ObjectDisposedException)
        {
            // It exited between the state check and Kill.
        }
        finally
        {
            process.Dispose();
        }
    }

    public void Dispose()
    {
        Stop();
    }

    private static async Task WaitUntilHealthyAsync(Process process, string baseUrl)
    {
        for (var attempt = 0; attempt < 80; attempt++)
        {
            if (process.HasExited)
            {
                throw new InvalidOperationException($"本地网关意外退出，退出码 {process.ExitCode}。请查看 %APPDATA%\\HeiGate\\logs\\desktop.log。");
            }

            try
            {
                using var response = await HttpClient.GetAsync($"{baseUrl}/health");
                if (response.StatusCode == HttpStatusCode.OK)
                {
                    await using var stream = await response.Content.ReadAsStreamAsync();
                    using var payload = await JsonDocument.ParseAsync(stream);
                    if (payload.RootElement.TryGetProperty("mode", out var mode) &&
                        mode.GetString() == "desktop")
                    {
                        return;
                    }
                }
            }
            catch (HttpRequestException)
            {
                // The process is still starting.
            }
            catch (TaskCanceledException)
            {
                // Retry until the overall startup window expires.
            }
            await Task.Delay(250);
        }

        throw new TimeoutException("本地网关启动超时。请查看 %APPDATA%\\HeiGate\\logs\\desktop.log。");
    }

    private static int FindAvailablePort()
    {
        for (var port = 18080; port <= 18090; port++)
        {
            if (CanBind(port))
            {
                return port;
            }
        }

        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var portNumber = ((IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();
        return portNumber;
    }

    private static bool CanBind(int port)
    {
        var listener = new TcpListener(IPAddress.Loopback, port);
        try
        {
            listener.Start();
            return true;
        }
        catch (SocketException)
        {
            return false;
        }
        finally
        {
            listener.Stop();
        }
    }

    private static void AppendLog(string path, string? line)
    {
        if (string.IsNullOrWhiteSpace(line))
        {
            return;
        }
        try
        {
            lock (typeof(GatewayProcess))
            {
                File.AppendAllText(path, $"{DateTimeOffset.Now:O} {line}{Environment.NewLine}");
            }
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            // Logging must never take the gateway down.
        }
    }
}

internal static class AutoStart
{
    private const string RegistryPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string ValueName = "HeiGate";

    public static bool IsEnabled
    {
        get
        {
            using var key = Registry.CurrentUser.OpenSubKey(RegistryPath, writable: false);
            return key?.GetValue(ValueName) is string value && !string.IsNullOrWhiteSpace(value);
        }
    }

    public static void SetEnabled(bool enabled)
    {
        using var key = Registry.CurrentUser.CreateSubKey(RegistryPath, writable: true)
            ?? throw new InvalidOperationException("无法打开 Windows 启动项注册表。");
        if (enabled)
        {
            key.SetValue(ValueName, $"\"{Application.ExecutablePath}\"");
        }
        else
        {
            key.DeleteValue(ValueName, throwOnMissingValue: false);
        }
    }
}
