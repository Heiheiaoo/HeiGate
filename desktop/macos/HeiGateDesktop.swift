import Cocoa
import WebKit
import Foundation
import Darwin

final class WindowActionHandler: NSObject, WKScriptMessageHandler {
    weak var window: NSWindow?
    static var lastMouseDownEvent: NSEvent?

    init(window: NSWindow?) {
        self.window = window
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let window = window else { return }
        if message.name == "dragWindow" {
            let event = WindowActionHandler.lastMouseDownEvent ?? NSApp.currentEvent
            if let event = event {
                window.performDrag(with: event)
            }
        } else if message.name == "zoomWindow" {
            window.zoom(nil)
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate, WKUIDelegate, NSWindowDelegate {
    private var server: Process?
    private var window: NSWindow?
    private var webView: WKWebView?
    private var statusItem: NSStatusItem?
    private var autoStartMenuItem: NSMenuItem?
    private var stoppingServer = false
    private var port = 18080
    private let launchAgentLabel = "com.heigate.desktop.autostart"
    private let windowFrameKey = "HeiGateMainWindowFrame"

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let iconURL = Bundle.main.url(forResource: "AppIcon", withExtension: "icns"),
           let iconImage = NSImage(contentsOf: iconURL) {
            NSApp.applicationIconImage = iconImage
        }

        port = findAvailablePort()
        setupMainMenu()
        setupStatusItem()

        NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown]) { event in
            WindowActionHandler.lastMouseDownEvent = event
            return event
        }

        var hasRestoredFrame = false
        var initialRect = NSRect(x: 0, y: 0, width: 1440, height: 920)

        if let savedString = UserDefaults.standard.string(forKey: windowFrameKey) {
            let savedRect = NSRectFromString(savedString)
            if savedRect.width >= 1000 && savedRect.height >= 680 {
                let isVisible = NSScreen.screens.contains { screen in
                    NSIntersectionRect(screen.visibleFrame, savedRect).width >= 200 &&
                    NSIntersectionRect(screen.visibleFrame, savedRect).height >= 200
                }
                if isVisible {
                    initialRect = savedRect
                    hasRestoredFrame = true
                }
            }
        }

        let appWindow = NSWindow(
            contentRect: initialRect,
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        appWindow.minSize = NSSize(width: 1000, height: 680)
        appWindow.title = ""
        appWindow.titleVisibility = .hidden
        appWindow.titlebarAppearsTransparent = true
        appWindow.titlebarSeparatorStyle = .none
        appWindow.isMovableByWindowBackground = true
        appWindow.delegate = self

        if !hasRestoredFrame {
            appWindow.center()
        }

        let configuration = WKWebViewConfiguration()
        let actionHandler = WindowActionHandler(window: appWindow)
        configuration.userContentController.add(actionHandler, name: "dragWindow")
        configuration.userContentController.add(actionHandler, name: "zoomWindow")

        let view = WKWebView(frame: .zero, configuration: configuration)
        view.allowsBackForwardNavigationGestures = true
        view.uiDelegate = self
        webView = view

        appWindow.contentView = view
        appWindow.isReleasedWhenClosed = false
        appWindow.makeKeyAndOrderFront(nil)
        window = appWindow

        startServer()
        waitForServer(attempt: 0)
    }

    private func persistWindowFrame(_ win: NSWindow) {
        guard !win.isMiniaturized else { return }
        let frameString = NSStringFromRect(win.frame)
        UserDefaults.standard.set(frameString, forKey: windowFrameKey)
        UserDefaults.standard.synchronize()
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        if let win = notification.object as? NSWindow {
            persistWindowFrame(win)
        }
    }

    func windowDidMove(_ notification: Notification) {
        if let win = notification.object as? NSWindow {
            persistWindowFrame(win)
        }
    }

    func windowWillClose(_ notification: Notification) {
        if let win = notification.object as? NSWindow {
            persistWindowFrame(win)
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        if let win = window {
            persistWindowFrame(win)
        }
        stoppingServer = true
        stopServer()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    @objc private func showWindow(_ sender: Any?) {
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func restartServer(_ sender: Any?) {
        stoppingServer = true
        stopServer()
        stoppingServer = false
        startServer()
        waitForServer(attempt: 0)
    }

    @objc private func quitApplication(_ sender: Any?) {
        NSApp.terminate(nil)
    }

    @objc private func menuNewChannel(_ sender: Any?) {
        showWindow(sender)
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'new-channel' }))", completionHandler: nil)
    }

    @objc private func menuEditChannel(_ sender: Any?) {
        showWindow(sender)
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'edit-first-channel' }))", completionHandler: nil)
    }

    @objc private func menuSearch(_ sender: Any?) {
        showWindow(sender)
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'search' }))", completionHandler: nil)
    }

    @objc private func menuRefresh(_ sender: Any?) {
        showWindow(sender)
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'refresh' }))", completionHandler: nil)
    }

    @objc private func menuSettings(_ sender: Any?) {
        showWindow(sender)
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'settings' }))", completionHandler: nil)
    }

    @objc private func menuProbeAll(_ sender: Any?) {
        showWindow(sender)
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'probe-all' }))", completionHandler: nil)
    }

    @objc private func menuShortcutsHelp(_ sender: Any?) {
        showWindow(sender)
        webView?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('desktop-shortcut', { detail: 'shortcuts-help' }))", completionHandler: nil)
    }

    private func setupMainMenu() {
        let mainMenu = NSMenu()

        // 1. Application menu
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        let appName = "HeiGate"
        appMenu.addItem(withTitle: "关于 \(appName)", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        let prefsItem = appMenu.addItem(withTitle: "偏好设置...", action: #selector(menuSettings(_:)), keyEquivalent: ",")
        prefsItem.target = self
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "隐藏 \(appName)", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let hideOthersItem = appMenu.addItem(withTitle: "隐藏其他", action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h")
        hideOthersItem.keyEquivalentModifierMask = [.command, .option]
        appMenu.addItem(withTitle: "显示全部", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "退出 \(appName)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)

        // 2. File menu (Standard desktop shortcuts: ⌘N, ⌘E, ⌘K, ⌘R, ⌘⇧P)
        let fileMenuItem = NSMenuItem()
        let fileMenu = NSMenu(title: "文件")
        let newItem = fileMenu.addItem(withTitle: "新建渠道...", action: #selector(menuNewChannel(_:)), keyEquivalent: "n")
        newItem.target = self
        let editItem = fileMenu.addItem(withTitle: "编辑渠道...", action: #selector(menuEditChannel(_:)), keyEquivalent: "e")
        editItem.target = self
        let searchItem = fileMenu.addItem(withTitle: "聚焦搜索", action: #selector(menuSearch(_:)), keyEquivalent: "k")
        searchItem.target = self
        let probeAllItem = fileMenu.addItem(withTitle: "批量探活全部渠道", action: #selector(menuProbeAll(_:)), keyEquivalent: "p")
        probeAllItem.keyEquivalentModifierMask = [.command, .shift]
        probeAllItem.target = self
        fileMenu.addItem(.separator())
        let refreshItem = fileMenu.addItem(withTitle: "刷新状态", action: #selector(menuRefresh(_:)), keyEquivalent: "r")
        refreshItem.target = self
        fileMenuItem.submenu = fileMenu
        mainMenu.addItem(fileMenuItem)

        // 3. Edit menu (Standard macOS clipboard & text editing responder shortcuts: ⌘C, ⌘V, ⌘X, ⌘A, ⌘Z)
        let editMenuItem = NSMenuItem()
        let editMenu = NSMenu(title: "编辑")
        editMenu.addItem(withTitle: "撤销", action: #selector(UndoManager.undo), keyEquivalent: "z")
        let redoItem = editMenu.addItem(withTitle: "重做", action: #selector(UndoManager.redo), keyEquivalent: "Z")
        redoItem.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "剪切", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "复制", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "粘贴", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        let pastePlainItem = editMenu.addItem(withTitle: "粘贴并匹配样式", action: #selector(NSTextView.pasteAsPlainText(_:)), keyEquivalent: "V")
        pastePlainItem.keyEquivalentModifierMask = [.command, .option, .shift]
        editMenu.addItem(withTitle: "删除", action: #selector(NSText.delete(_:)), keyEquivalent: "")
        editMenu.addItem(withTitle: "全选", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenuItem.submenu = editMenu
        mainMenu.addItem(editMenuItem)

        // 4. Window menu
        let windowMenuItem = NSMenuItem()
        let windowMenu = NSMenu(title: "窗口")
        windowMenu.addItem(withTitle: "最小化", action: #selector(NSWindow.miniaturize(_:)), keyEquivalent: "m")
        windowMenu.addItem(withTitle: "缩放", action: #selector(NSWindow.zoom(_:)), keyEquivalent: "")
        windowMenu.addItem(.separator())
        windowMenu.addItem(withTitle: "关闭窗口", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        windowMenuItem.submenu = windowMenu
        mainMenu.addItem(windowMenuItem)

        // 5. Help menu
        let helpMenuItem = NSMenuItem()
        let helpMenu = NSMenu(title: "帮助")
        let shortcutsHelpItem = helpMenu.addItem(withTitle: "键盘快捷键速查...", action: #selector(menuShortcutsHelp(_:)), keyEquivalent: "/")
        shortcutsHelpItem.target = self
        helpMenuItem.submenu = helpMenu
        mainMenu.addItem(helpMenuItem)

        NSApp.mainMenu = mainMenu
    }

    private func createStatusItemImage() -> NSImage {
        let icon = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { rect in
            // Outer Squircle Border (白色边框，系统模板自适应)
            let inset: CGFloat = 1.0
            let bRect = CGRect(x: inset, y: inset, width: rect.width - inset * 2, height: rect.height - inset * 2)
            let bPath = NSBezierPath(roundedRect: NSRect(origin: bRect.origin, size: bRect.size), xRadius: 4.2, yRadius: 4.2)
            bPath.lineWidth = 1.3
            NSColor.black.setStroke()
            bPath.stroke()

            // Letter H
            let hLeftX: CGFloat = 4.8
            let hRightX: CGFloat = 7.6
            let hTopY: CGFloat = 13.0
            let hBotY: CGFloat = 5.2
            let hMidY: CGFloat = 8.8

            let hPath = NSBezierPath()
            hPath.move(to: NSPoint(x: hLeftX, y: hBotY))
            hPath.line(to: NSPoint(x: hLeftX, y: hTopY - 0.8))
            hPath.move(to: NSPoint(x: hRightX, y: hBotY + 1.2))
            hPath.line(to: NSPoint(x: hRightX, y: hTopY))
            hPath.move(to: NSPoint(x: hLeftX, y: hMidY))
            hPath.line(to: NSPoint(x: hRightX, y: hMidY))
            hPath.lineWidth = 1.3
            hPath.lineCapStyle = .round
            hPath.stroke()

            // Letter G
            let gPath = NSBezierPath()
            gPath.move(to: NSPoint(x: 13.2, y: 11.8))
            gPath.line(to: NSPoint(x: 9.8, y: 13.4))
            gPath.line(to: NSPoint(x: 9.8, y: 5.6))
            gPath.line(to: NSPoint(x: 13.2, y: 3.8))
            gPath.line(to: NSPoint(x: 13.2, y: 8.8))
            gPath.line(to: NSPoint(x: 11.2, y: 8.8))
            gPath.lineWidth = 1.3
            gPath.lineCapStyle = .round
            gPath.lineJoinStyle = .round
            gPath.stroke()

            return true
        }
        icon.isTemplate = true
        return icon
    }

    private func setupStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = item.button {
            button.image = createStatusItemImage()
            button.imagePosition = .imageOnly
            button.toolTip = "HeiGate 本地网关"
        }
        let menu = NSMenu()
        menu.addItem(withTitle: "显示窗口", action: #selector(showWindow(_:)), keyEquivalent: "")
        menu.addItem(withTitle: "重启本地网关", action: #selector(restartServer(_:)), keyEquivalent: "")
        let autoStartItem = menu.addItem(withTitle: "登录时自动启动", action: #selector(toggleAutoStart(_:)), keyEquivalent: "")
        autoStartItem.state = isAutoStartEnabled ? .on : .off
        self.autoStartMenuItem = autoStartItem
        menu.addItem(.separator())
        menu.addItem(withTitle: "退出 HeiGate", action: #selector(quitApplication(_:)), keyEquivalent: "q")
        for menuItem in menu.items {
            menuItem.target = self
        }
        item.menu = menu
        statusItem = item
    }

    private var launchAgentURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
            .appendingPathComponent("\(launchAgentLabel).plist")
    }

    private var isAutoStartEnabled: Bool {
        FileManager.default.fileExists(atPath: launchAgentURL.path)
    }

    @objc private func toggleAutoStart(_ sender: NSMenuItem) {
        let enable = !isAutoStartEnabled
        do {
            if enable {
                let directory = launchAgentURL.deletingLastPathComponent()
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                let payload: [String: Any] = [
                    "Label": launchAgentLabel,
                    "ProgramArguments": [Bundle.main.executableURL?.path ?? ""],
                    "RunAtLoad": true,
                ]
                let data = try PropertyListSerialization.data(fromPropertyList: payload, format: .xml, options: 0)
                try data.write(to: launchAgentURL, options: .atomic)
                runLaunchctl(["bootout", "gui/\(getuid())", launchAgentURL.path])
                runLaunchctl(["bootstrap", "gui/\(getuid())", launchAgentURL.path])
            } else {
                runLaunchctl(["bootout", "gui/\(getuid())", launchAgentURL.path])
                try? FileManager.default.removeItem(at: launchAgentURL)
            }
            autoStartMenuItem?.state = isAutoStartEnabled ? .on : .off
        } catch {
            let alert = NSAlert()
            alert.messageText = "HeiGate"
            alert.informativeText = "无法更新登录自启动设置：\(error.localizedDescription)"
            alert.alertStyle = .warning
            alert.addButton(withTitle: "确定")
            alert.runModal()
        }
    }

    private func runLaunchctl(_ arguments: [String]) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        process.arguments = arguments
        try? process.run()
        process.waitUntilExit()
    }

    private func stopServer() {
        guard let server else { return }
        if server.isRunning {
            server.terminate()
            server.waitUntilExit()
        }
        self.server = nil
    }

    private func startServer() {
        let binaryURL = Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/heigate-desktop-bin")
        let dataURL = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("HeiGate", isDirectory: true)
        try? FileManager.default.createDirectory(at: dataURL, withIntermediateDirectories: true)

        let process = Process()
        process.executableURL = binaryURL
        process.arguments = ["-desktop"]
        var env = ProcessInfo.processInfo.environment
        env["DESKTOP_ONLY"] = "true"
        env["DESKTOP_SERVER_HOST"] = "127.0.0.1"
        env["DESKTOP_SERVER_PORT"] = String(port)
        env["DATA_DIR"] = dataURL.path
        process.environment = env
        do {
            process.terminationHandler = { [weak self] _ in
                DispatchQueue.main.async {
                    guard let self, !self.stoppingServer else { return }
                    self.startServer()
                    self.waitForServer(attempt: 0)
                }
            }
            try process.run()
            server = process
        } catch {
            showError("无法启动本地网关：\(error.localizedDescription)")
        }
    }

    private func findAvailablePort() -> Int {
        for candidatePort in 18080...18090 {
            let socketFD = socket(AF_INET, SOCK_STREAM, 0)
            guard socketFD >= 0 else { continue }
            var reuse: Int32 = 1
            setsockopt(socketFD, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))

            var address = sockaddr_in()
            address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            address.sin_family = sa_family_t(AF_INET)
            address.sin_port = in_port_t(candidatePort).bigEndian
            address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))

            let bindResult = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(socketFD, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
            close(socketFD)
            if bindResult == 0 {
                return candidatePort
            }
        }

        let socketFD = socket(AF_INET, SOCK_STREAM, 0)
        guard socketFD >= 0 else { return 18080 }
        defer { close(socketFD) }

        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = 0
        address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
        let bindResult = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(socketFD, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bindResult == 0 else { return 18080 }

        var boundAddress = sockaddr_in()
        var addressLength = socklen_t(MemoryLayout<sockaddr_in>.size)
        let nameResult = withUnsafeMutablePointer(to: &boundAddress) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(socketFD, $0, &addressLength)
            }
        }
        guard nameResult == 0 else { return 18080 }
        return Int(UInt16(bigEndian: boundAddress.sin_port))
    }

    private func waitForServer(attempt: Int) {
        if attempt > 80 {
            showError("本地网关启动超时，请检查应用日志。")
            return
        }
        let healthURL = URL(string: "http://127.0.0.1:\(port)/health")!
        URLSession.shared.dataTask(with: healthURL) { [weak self] data, response, _ in
            DispatchQueue.main.async {
                guard let self else { return }
                if let http = response as? HTTPURLResponse, http.statusCode == 200 {
                    // A healthy response from another local service must not be
                    // mistaken for the desktop gateway.
                    let payload = (try? JSONSerialization.jsonObject(with: data ?? Data())) as? [String: Any]
                    let isDesktop = payload?["mode"] as? String == "desktop"
                    guard isDesktop else {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                            self.waitForServer(attempt: attempt + 1)
                        }
                        return
                    }
                    let url = URL(string: "http://127.0.0.1:\(self.port)/desktop")!
                    self.webView?.load(URLRequest(url: url))
                } else {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                        self.waitForServer(attempt: attempt + 1)
                    }
                }
            }
        }.resume()
    }

    private func showError(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "HeiGate"
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: "退出")
        alert.runModal()
        NSApp.terminate(nil)
    }

    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping () -> Void) {
        let alert = NSAlert()
        alert.messageText = "HeiGate"
        alert.informativeText = message
        alert.alertStyle = .informational
        alert.addButton(withTitle: "确定")
        if let window = self.window {
            alert.beginSheetModal(for: window) { _ in completionHandler() }
        } else {
            alert.runModal()
            completionHandler()
        }
    }

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void) {
        let alert = NSAlert()
        alert.messageText = "HeiGate"
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: "确定")
        alert.addButton(withTitle: "取消")
        if let window = self.window {
            alert.beginSheetModal(for: window) { response in
                completionHandler(response == .alertFirstButtonReturn)
            }
        } else {
            let res = alert.runModal()
            completionHandler(res == .alertFirstButtonReturn)
        }
    }
}

let application = NSApplication.shared
let delegate = AppDelegate()
application.delegate = delegate
application.setActivationPolicy(.regular)
application.activate(ignoringOtherApps: true)
application.run()
