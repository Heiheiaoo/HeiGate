package desktopapi

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/gin-gonic/gin"
)

type ClientType string

const (
	ClientTypeClaude ClientType = "claude"
	ClientTypeCodex  ClientType = "codex"
)

type ClaudeModelMappingItem struct {
	DesktopSlot   string `json:"desktop_slot"`   // e.g. "claude-sonnet-4-6"
	UpstreamModel string `json:"upstream_model"` // e.g. "MiniMaxAI/MiniMax-M3"
	LabelOverride string `json:"label_override"` // e.g. "MiniMaxAI/MiniMax-M3"
	Supports1M    bool   `json:"supports_1m"`
}

type ClientStatus struct {
	Type             ClientType               `json:"type"`
	Name             string                   `json:"name"`
	Installed        bool                     `json:"installed"`
	AppPath          string                   `json:"app_path"`
	Running          bool                     `json:"running"`
	Configured       bool                     `json:"configured"`
	GatewayURL       string                   `json:"gateway_url"`
	ConfigPath       string                   `json:"config_path"`
	ConfiguredModels []string                 `json:"configured_models"`
	DefaultModel     string                   `json:"default_model"`
	Mappings         []ClaudeModelMappingItem `json:"mappings,omitempty"`
}

var defaultClaudeSlots = []struct {
	Slot string
	Desc string
}{
	{Slot: "claude-sonnet-4-6", Desc: "Sonnet 4.6 (主力模型)"},
	{Slot: "claude-opus-4-8", Desc: "Opus 4.8 (超强推理)"},
	{Slot: "claude-haiku-4-5", Desc: "Haiku 4.5 (轻量极速)"},
	{Slot: "claude-fable-5", Desc: "Fable 5 (旗舰模型)"},
	{Slot: "claude-opus-4-6", Desc: "Opus 4.6"},
	{Slot: "claude-opus-4-7", Desc: "Opus 4.7"},
	{Slot: "claude-sonnet-4-6-r2", Desc: "Sonnet 4.6 R2"},
}

func (h *Handler) getGatewayPort() string {
	port := strings.TrimSpace(os.Getenv("DESKTOP_SERVER_PORT"))
	if port == "" {
		port = "8080"
	}
	return port
}

func (h *Handler) getGatewayBaseURL(c *gin.Context) string {
	port := h.getGatewayPort()
	return fmt.Sprintf("http://127.0.0.1:%s", port)
}

func userHomeDir() string {
	if home, err := os.UserHomeDir(); err == nil && home != "" && filepath.IsAbs(home) {
		return home
	}
	if home := os.Getenv("HOME"); home != "" && filepath.IsAbs(home) {
		return home
	}
	if u, err := user.Current(); err == nil && u.HomeDir != "" && filepath.IsAbs(u.HomeDir) {
		return u.HomeDir
	}
	if username := os.Getenv("USER"); username != "" {
		candidate := filepath.Join("/Users", username)
		if _, err := os.Stat(candidate); err == nil {
			return candidate
		}
	}
	if _, err := os.Stat("/Users/heihei"); err == nil {
		return "/Users/heihei"
	}
	return "/"
}

func isProcessRunning(pattern string) bool {
	cmd := exec.Command("pgrep", "-f", pattern)
	err := cmd.Run()
	return err == nil
}

const heigateClaudeProfileUUID = "7e77e954-a6cb-4962-9c5b-138828fcb441"

func (h *Handler) inspectClaudeClient(ctx context.Context) ClientStatus {
	appPath := "/Applications/Claude.app"
	installed := false
	if _, err := os.Stat(appPath); err == nil {
		installed = true
	}
	running := isProcessRunning("Claude.app/Contents/MacOS/Claude") || isProcessRunning("/Applications/Claude.app")

	home := userHomeDir()
	threepDir := filepath.Join(home, "Library", "Application Support", "Claude-3p")
	metaPath := filepath.Join(threepDir, "configLibrary", "_meta.json")
	heigateProfilePath := filepath.Join(threepDir, "configLibrary", heigateClaudeProfileUUID+".json")
	if _, err := os.Stat(heigateProfilePath); err != nil {
		heigateProfilePath = filepath.Join(threepDir, "configLibrary", "heigate-gateway.json")
	}

	configured := false
	var configuredModels []string
	var mappings []ClaudeModelMappingItem

	if data, err := os.ReadFile(heigateProfilePath); err == nil {
		var cfg struct {
			InferenceGatewayBaseURL string `json:"inferenceGatewayBaseUrl"`
			InferenceModels         []struct {
				Name          string `json:"name"`
				LabelOverride string `json:"labelOverride"`
				Supports1M    bool   `json:"supports1m"`
			} `json:"inferenceModels"`
		}
		if json.Unmarshal(data, &cfg) == nil {
			if strings.Contains(cfg.InferenceGatewayBaseURL, "127.0.0.1") || strings.Contains(cfg.InferenceGatewayBaseURL, "localhost") {
				configured = true
				for _, m := range cfg.InferenceModels {
					display := m.LabelOverride
					if display == "" {
						display = m.Name
					}
					configuredModels = append(configuredModels, display)
					mappings = append(mappings, ClaudeModelMappingItem{
						DesktopSlot:   m.Name,
						UpstreamModel: display,
						LabelOverride: display,
						Supports1M:    m.Supports1M,
					})
				}
			}
		}
	}

	return ClientStatus{
		Type:             ClientTypeClaude,
		Name:             "Claude 桌面端",
		Installed:        installed,
		AppPath:          appPath,
		Running:          running,
		Configured:       configured,
		GatewayURL:       fmt.Sprintf("http://127.0.0.1:%s", h.getGatewayPort()),
		ConfigPath:       metaPath,
		ConfiguredModels: configuredModels,
		Mappings:         mappings,
	}
}

func (h *Handler) inspectCodexClient(ctx context.Context) ClientStatus {
	appPath := "/Applications/ChatGPT.app"
	installed := false
	if _, err := os.Stat(appPath); err == nil {
		installed = true
	} else if _, err := exec.LookPath("codex"); err == nil {
		installed = true
		appPath = "codex (CLI)"
	}
	running := isProcessRunning("ChatGPT.app") || isProcessRunning("Codex.app")

	home := userHomeDir()
	configPath := filepath.Join(home, ".codex", "config.toml")
	configured := false
	defaultModel := ""
	var configuredModels []string

	if data, err := os.ReadFile(configPath); err == nil {
		content := string(data)
		if strings.Contains(content, "127.0.0.1") || strings.Contains(content, "localhost") {
			configured = true
		}
		reModel := regexp.MustCompile(`(?m)^model\s*=\s*"([^"]+)"`)
		if matches := reModel.FindStringSubmatch(content); len(matches) > 1 {
			defaultModel = matches[1]
		}
	}

	catalogPath := filepath.Join(home, ".codex", "heigate-model-catalog.json")
	if data, err := os.ReadFile(catalogPath); err == nil {
		var catalog struct {
			Models []struct {
				Slug        string `json:"slug"`
				DisplayName string `json:"display_name"`
			} `json:"models"`
		}
		if json.Unmarshal(data, &catalog) == nil {
			for _, m := range catalog.Models {
				name := m.DisplayName
				if name == "" {
					name = m.Slug
				}
				if name != "" {
					configuredModels = append(configuredModels, name)
				}
			}
		}
	}
	if len(configuredModels) == 0 && defaultModel != "" {
		configuredModels = append(configuredModels, defaultModel)
	}

	return ClientStatus{
		Type:             ClientTypeCodex,
		Name:             "Codex 桌面端 (ChatGPT / Codex)",
		Installed:        installed,
		AppPath:          appPath,
		Running:          running,
		Configured:       configured,
		GatewayURL:       fmt.Sprintf("http://127.0.0.1:%s/v1", h.getGatewayPort()),
		ConfigPath:       configPath,
		ConfiguredModels: configuredModels,
		DefaultModel:     defaultModel,
	}
}

func (h *Handler) listClients(c *gin.Context) {
	claude := h.inspectClaudeClient(c.Request.Context())
	codex := h.inspectCodexClient(c.Request.Context())
	c.JSON(200, gin.H{
		"data": []ClientStatus{claude, codex},
	})
}

type ConfigureClaudeRequest struct {
	Models   []string                 `json:"models"`
	Mappings []ClaudeModelMappingItem `json:"mappings"`
}

func (h *Handler) writeClaudeDesktopConfig(ctx context.Context, baseURL string, models []string, mappingItems []ClaudeModelMappingItem) error {
	home := userHomeDir()
	threepDir := filepath.Join(home, "Library", "Application Support", "Claude-3p")
	normalDir := filepath.Join(home, "Library", "Application Support", "Claude")
	configLibDir := filepath.Join(threepDir, "configLibrary")

	if err := os.MkdirAll(configLibDir, 0o755); err != nil {
		return fmt.Errorf("创建 Claude-3p 配置目录失败: %w", err)
	}
	_ = os.MkdirAll(normalDir, 0o755)

	setDeploymentMode := func(dir string) error {
		_ = os.MkdirAll(dir, 0o755)
		cfgFile := filepath.Join(dir, "claude_desktop_config.json")
		var obj map[string]any
		if data, err := os.ReadFile(cfgFile); err == nil {
			_ = json.Unmarshal(data, &obj)
		}
		if obj == nil {
			obj = make(map[string]any)
		}
		obj["deploymentMode"] = "3p"
		bytes, err := json.MarshalIndent(obj, "", "  ")
		if err != nil {
			return err
		}
		return os.WriteFile(cfgFile, bytes, 0o600)
	}

	_ = setDeploymentMode(threepDir)
	_ = setDeploymentMode(normalDir)

	if len(mappingItems) == 0 {
		for i, m := range models {
			slot := ""
			if i < len(defaultClaudeSlots) {
				slot = defaultClaudeSlots[i].Slot
			} else {
				slot = fmt.Sprintf("claude-custom-%d", i+1)
			}
			mappingItems = append(mappingItems, ClaudeModelMappingItem{
				DesktopSlot:   slot,
				UpstreamModel: m,
				LabelOverride: m,
				Supports1M:    true,
			})
		}
	}

	type infModel struct {
		Name          string `json:"name"`
		LabelOverride string `json:"labelOverride"`
		Supports1M    bool   `json:"supports1m"`
	}
	var infModels []infModel
	settingMappings := make(map[string]string)

	for _, item := range mappingItems {
		infModels = append(infModels, infModel{
			Name:          item.DesktopSlot,
			LabelOverride: item.UpstreamModel,
			Supports1M:    true,
		})
		settingMappings[strings.ToLower(item.DesktopSlot)] = item.UpstreamModel
	}

	gatewayProfile := map[string]any{
		"coworkEgressAllowedHosts":    []string{"*"},
		"disableDeploymentModeChooser": true,
		"inferenceProvider":           "gateway",
		"inferenceGatewayBaseUrl":     baseURL,
		"inferenceGatewayApiKey":      h.key,
		"inferenceGatewayAuthScheme":  "bearer",
		"inferenceModels":             infModels,
	}

	profileBytes, err := json.MarshalIndent(gatewayProfile, "", "  ")
	if err != nil {
		return fmt.Errorf("生成 Claude Profile 配置失败: %w", err)
	}

	meta := map[string]any{
		"appliedId": heigateClaudeProfileUUID,
		"entries": []map[string]any{
			{
				"id":   heigateClaudeProfileUUID,
				"name": "HeiGate 本地网关",
			},
		},
	}
	metaBytes, err := json.MarshalIndent(meta, "", "  ")
	if err != nil {
		return fmt.Errorf("生成 Claude _meta.json 失败: %w", err)
	}

	// Write profile and meta to both Claude-3p and standard Claude directories
	targetDirs := []string{
		filepath.Join(threepDir, "configLibrary"),
		filepath.Join(normalDir, "configLibrary"),
	}
	for _, targetDir := range targetDirs {
		_ = os.MkdirAll(targetDir, 0o755)
			_ = os.WriteFile(filepath.Join(targetDir, heigateClaudeProfileUUID+".json"), profileBytes, 0o600)
			_ = os.WriteFile(filepath.Join(targetDir, "heigate-gateway.json"), profileBytes, 0o600)
		_ = os.WriteFile(filepath.Join(targetDir, "_meta.json"), metaBytes, 0o644)
	}

	if mappingsJSON, err := json.Marshal(settingMappings); err == nil && h.store != nil {
		_ = h.store.SetSetting(ctx, "claude_desktop_model_mappings", string(mappingsJSON))
	}

	return nil
}

func (h *Handler) configureClaude(c *gin.Context) {
	var req ConfigureClaudeRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(400, gin.H{"error": "invalid request body: " + err.Error()})
		return
	}

	models := req.Models
	if len(models) == 0 && len(req.Mappings) > 0 {
		for _, m := range req.Mappings {
			if m.UpstreamModel != "" {
				models = append(models, m.UpstreamModel)
			}
		}
	}
	if len(models) == 0 {
		c.JSON(400, gin.H{"error": "请至少选择一个要配置到 Claude 桌面端的模型"})
		return
	}

	baseURL := h.getGatewayBaseURL(c)
	if err := h.writeClaudeDesktopConfig(c.Request.Context(), baseURL, models, req.Mappings); err != nil {
		c.JSON(500, gin.H{"error": err.Error()})
		return
	}

	c.JSON(200, gin.H{
		"message": "Claude 桌面端配置写入成功！已就绪",
		"profile": heigateClaudeProfileUUID,
		"count":   len(models),
	})
}

func (h *Handler) launchClaude(c *gin.Context) {
	home := userHomeDir()
	threepDir := filepath.Join(home, "Library", "Application Support", "Claude-3p")
	normalDir := filepath.Join(home, "Library", "Application Support", "Claude")
	heigateProfilePath := filepath.Join(threepDir, "configLibrary", heigateClaudeProfileUUID+".json")

	// If profile does not exist yet, auto-provision it using active channels
	if _, err := os.Stat(heigateProfilePath); err != nil {
		var availableModels []string
		if h.store != nil {
			channels, _ := h.store.List(c.Request.Context())
			for _, ch := range channels {
				if ch.Enabled {
					if len(ch.ExtraModels) > 0 {
						availableModels = append(availableModels, ch.ExtraModels...)
					} else if ch.PrimaryModel != "" {
						availableModels = append(availableModels, ch.PrimaryModel)
					}
				}
			}
		}
		if len(availableModels) == 0 {
			availableModels = []string{"claude-3-5-sonnet-20241022"}
		}
		_ = h.writeClaudeDesktopConfig(c.Request.Context(), h.getGatewayBaseURL(c), availableModels, nil)
	} else {
		// Ensure 3p deployment mode is set
		for _, dir := range []string{threepDir, normalDir} {
			_ = os.MkdirAll(dir, 0o755)
			cfgFile := filepath.Join(dir, "claude_desktop_config.json")
			var obj map[string]any
			if data, err := os.ReadFile(cfgFile); err == nil {
				_ = json.Unmarshal(data, &obj)
			}
			if obj == nil {
				obj = make(map[string]any)
			}
			obj["deploymentMode"] = "3p"
			bytes, _ := json.MarshalIndent(obj, "", "  ")
			_ = os.WriteFile(cfgFile, bytes, 0o644)
		}
	}

	// 1. Gracefully kill any running/hung Claude instances so Electron reloads config
	_ = exec.Command("killall", "Claude", "Claude Helper").Run()
	time.Sleep(600 * time.Millisecond)

	// 2. Launch Claude.app natively via macOS LaunchServices
	cmd := exec.Command("open", "-a", "/Applications/Claude.app")
	if err := cmd.Run(); err != nil {
		_ = exec.Command("open", "-n", "-a", "/Applications/Claude.app").Run()
	}

	// 3. Bring window to front
	time.Sleep(400 * time.Millisecond)
	_ = exec.Command("osascript", "-e", `tell application "Claude" to activate`).Start()

	c.JSON(200, gin.H{"message": "Claude 桌面端已重新启动并应用配置"})
}

func (h *Handler) writeCodexConfig(baseURL string, models []string, defaultModel string) error {
	if len(models) == 0 {
		return fmt.Errorf("请至少选择一个模型配置到 Codex")
	}
	if defaultModel == "" {
		defaultModel = models[0]
	}

	home := userHomeDir()
	codexDir := filepath.Join(home, ".codex")
	if err := os.MkdirAll(codexDir, 0o755); err != nil {
		return fmt.Errorf("创建 ~/.codex 目录失败: %w", err)
	}

	configPath := filepath.Join(codexDir, "config.toml")
	var existingLines []string
	if data, err := os.ReadFile(configPath); err == nil {
		_ = os.WriteFile(configPath+".bak.heigate", data, 0o600)
		existingLines = strings.Split(string(data), "\n")
	}

	hasBaseURL := false
	hasModel := false
	hasCatalog := false

	var newLines []string
	inModelProviderSection := false
	for _, line := range existingLines {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "[model_providers.") {
			inModelProviderSection = true
			newLines = append(newLines, line)
			continue
		} else if strings.HasPrefix(trimmed, "[") {
			inModelProviderSection = false
		}

		if inModelProviderSection && strings.HasPrefix(trimmed, "base_url") {
			newLines = append(newLines, fmt.Sprintf("base_url = %q", baseURL))
		} else if inModelProviderSection && strings.HasPrefix(trimmed, "experimental_bearer_token") {
			newLines = append(newLines, fmt.Sprintf("experimental_bearer_token = %q", h.key))
		} else if inModelProviderSection && strings.HasPrefix(trimmed, "wire_api") {
			newLines = append(newLines, `wire_api = "chat"`)
		} else if strings.HasPrefix(trimmed, "openai_base_url") {
			newLines = append(newLines, fmt.Sprintf("openai_base_url = %q", baseURL))
			hasBaseURL = true
		} else if strings.HasPrefix(trimmed, "model =") || strings.HasPrefix(trimmed, "model=") {
			newLines = append(newLines, fmt.Sprintf("model = %q", defaultModel))
			hasModel = true
		} else if strings.HasPrefix(trimmed, "model_catalog_json") {
			newLines = append(newLines, `model_catalog_json = "heigate-model-catalog.json"`)
			hasCatalog = true
		} else {
			newLines = append(newLines, line)
		}
	}

	var headerLines []string
	if !hasBaseURL {
		headerLines = append(headerLines, fmt.Sprintf("openai_base_url = %q", baseURL))
	}
	if !hasModel {
		headerLines = append(headerLines, fmt.Sprintf("model = %q", defaultModel))
	}
	if !hasCatalog {
		headerLines = append(headerLines, `model_catalog_json = "heigate-model-catalog.json"`)
	}

	finalContent := strings.Join(append(headerLines, newLines...), "\n")
	if err := os.WriteFile(configPath, []byte(finalContent), 0o600); err != nil {
		return fmt.Errorf("写入 config.toml 失败: %w", err)
	}

	authPath := filepath.Join(codexDir, "auth.json")
	authObj := map[string]string{
		"OPENAI_API_KEY": h.key,
		"auth_mode":      "apikey",
	}
	authBytes, _ := json.MarshalIndent(authObj, "", "  ")
	if err := os.WriteFile(authPath, authBytes, 0o600); err != nil {
		return fmt.Errorf("写入 auth.json 失败: %w", err)
	}

	catalogPath := filepath.Join(codexDir, "heigate-model-catalog.json")
	var catalogModels []map[string]any
	for _, m := range models {
		catalogModels = append(catalogModels, map[string]any{
			"slug":                    m,
			"display_name":            m,
			"description":             fmt.Sprintf("HeiGate 调度: %s", m),
			"context_window":          1000000,
			"max_context_window":      1000000,
			"default_reasoning_level": "high",
			"input_modalities":        []string{"text", "image"},
			"service_tiers":           []string{"priority"},
		})
	}
	catalogObj := map[string]any{
		"models": catalogModels,
	}
	catalogBytes, _ := json.MarshalIndent(catalogObj, "", "  ")
	if err := os.WriteFile(catalogPath, catalogBytes, 0o644); err != nil {
		return fmt.Errorf("写入 heigate-model-catalog.json 失败: %w", err)
	}
	return nil
}

type ConfigureCodexRequest struct {
	Models       []string `json:"models"`
	DefaultModel string   `json:"default_model"`
}

func (h *Handler) configureCodex(c *gin.Context) {
	var req ConfigureCodexRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(400, gin.H{"error": "invalid request body: " + err.Error()})
		return
	}

	baseURL := fmt.Sprintf("%s/v1", h.getGatewayBaseURL(c))
	defaultModel := req.DefaultModel
	if defaultModel == "" && len(req.Models) > 0 {
		defaultModel = req.Models[0]
	}

	if err := h.writeCodexConfig(baseURL, req.Models, defaultModel); err != nil {
		c.JSON(500, gin.H{"error": err.Error()})
		return
	}

	c.JSON(200, gin.H{
		"message":       "Codex 桌面端配置写入成功！已就绪",
		"default_model": defaultModel,
		"count":         len(req.Models),
	})
}

func (h *Handler) launchCodex(c *gin.Context) {
	home := userHomeDir()
	configPath := filepath.Join(home, ".codex", "config.toml")
	if _, err := os.Stat(configPath); err != nil {
		var availableModels []string
		if h.store != nil {
			channels, _ := h.store.List(c.Request.Context())
			for _, ch := range channels {
				if ch.Enabled {
					if len(ch.ExtraModels) > 0 {
						availableModels = append(availableModels, ch.ExtraModels...)
					} else if ch.PrimaryModel != "" {
						availableModels = append(availableModels, ch.PrimaryModel)
					}
				}
			}
		}
		if len(availableModels) == 0 {
			availableModels = []string{"gpt-4o"}
		}
		baseURL := fmt.Sprintf("%s/v1", h.getGatewayBaseURL(c))
		_ = h.writeCodexConfig(baseURL, availableModels, availableModels[0])
	}

	appPath := "/Applications/ChatGPT.app"
	if _, err := os.Stat(appPath); err == nil {
		_ = exec.Command("killall", "ChatGPT").Run()
		time.Sleep(500 * time.Millisecond)
		cmd := exec.Command("open", "-a", appPath)
		if err := cmd.Start(); err != nil {
			c.JSON(500, gin.H{"error": "调起 ChatGPT.app 失败: " + err.Error()})
			return
		}
		time.Sleep(400 * time.Millisecond)
		_ = exec.Command("osascript", "-e", `tell application "ChatGPT" to activate`).Start()
		c.JSON(200, gin.H{"message": "Codex 桌面端 (ChatGPT.app) 已重新启动并应用配置"})
		return
	}
	// Fallback to launching terminal with codex
	cmd := exec.Command("open", "-a", "Terminal")
	if err := cmd.Start(); err != nil {
		c.JSON(500, gin.H{"error": "调起 Terminal 失败: " + err.Error()})
		return
	}
	c.JSON(200, gin.H{"message": "已打开终端，可直接运行 codex"})
}
