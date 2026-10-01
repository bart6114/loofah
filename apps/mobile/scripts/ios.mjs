import { execFileSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const mobile = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(mobile, "../..");
const apple = resolve(mobile, "src-tauri/gen/apple");
const [command, ...args] = process.argv.slice(2);

function run(program, arguments_, cwd = mobile) {
  const result = spawnSync(program, arguments_, { cwd, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function prepare() {
  const iconSet = resolve(apple, "Assets.xcassets/AppIcon.appiconset");
  rmSync(iconSet, { recursive: true, force: true });
  mkdirSync(iconSet, { recursive: true });
  copyFileSync(
    resolve(root, "apps/desktop/src-tauri/icons/src/loofah-fullbleed-1024.png"),
    resolve(iconSet, "AppIcon.png"),
  );
  writeFileSync(
    resolve(iconSet, "Contents.json"),
    `${JSON.stringify(
      {
        images: [
          {
            filename: "AppIcon.png",
            idiom: "universal",
            platform: "ios",
            size: "1024x1024",
          },
        ],
        info: { author: "xcode", version: 1 },
      },
      null,
      2,
    )}\n`,
  );
  const config = JSON.parse(
    readFileSync(resolve(mobile, "src-tauri/tauri.conf.json"), "utf8"),
  );
  const projectPath = resolve(apple, "project.yml");
  const project = readFileSync(projectPath, "utf8")
    .replace(
      /^  bundleIdPrefix: .*$/m,
      `  bundleIdPrefix: ${config.identifier}`,
    )
    .replace(
      /^      PRODUCT_NAME: .*$/m,
      `      PRODUCT_NAME: ${config.productName}`,
    )
    .replace(
      /^      PRODUCT_BUNDLE_IDENTIFIER: .*$/m,
      `      PRODUCT_BUNDLE_IDENTIFIER: ${config.identifier}`,
    )
    .replace(/^      - path: Externals\n/m, "");
  writeFileSync(projectPath, project);
  const developmentTeam =
    process.env.APPLE_DEVELOPMENT_TEAM ?? config.bundle?.iOS?.developmentTeam;
  const appInfo = JSON.parse(
    execFileSync(
      "plutil",
      ["-convert", "json", "-o", "-", resolve(mobile, "src-tauri/Info.plist")],
      {
        encoding: "utf8",
      },
    ),
  );
  const version = JSON.parse(
    readFileSync(resolve(root, "package.json"), "utf8"),
  ).version;
  const buildNumber = process.env.MOBILE_BUILD_NUMBER || version;
  const source = (path) => ({ path: relative(apple, resolve(root, path)) });
  const intent = source("apps/mobile/ios/Shared/StopRecordingIntent.swift");
  const spec = {
    include: ["project.yml"],
    settingGroups: {
      app: {
        base: developmentTeam ? { DEVELOPMENT_TEAM: developmentTeam } : {},
      },
    },
    targets: {
      mobile_iOS: {
        sources: [intent],
        dependencies: [{ target: "RecordingWidget", embed: true }],
        info: {
          properties: {
            ...appInfo,
            NSSupportsLiveActivities: true,
            CFBundleShortVersionString: version,
            CFBundleVersion: buildNumber,
          },
        },
      },
      RecordingWidget: {
        type: "app-extension",
        platform: "iOS",
        deploymentTarget: "26.0",
        sources: [
          {
            ...source("apps/mobile/ios/RecordingWidget"),
            excludes: ["Info.plist"],
          },
          source(
            "plugins/mobile-native/ios/Sources/RecordingActivityAttributes.swift",
          ),
          intent,
        ],
        settings: {
          groups: ["app"],
          base: {
            PRODUCT_BUNDLE_IDENTIFIER: "io.loofah.notes.RecordingWidget",
            PRODUCT_NAME: "RecordingWidget",
            SWIFT_VERSION: "5.0",
            APPLICATION_EXTENSION_API_ONLY: true,
            SKIP_INSTALL: true,
            TARGETED_DEVICE_FAMILY: "1,2",
          },
        },
        info: {
          path: "RecordingWidget/Info.plist",
          properties: {
            CFBundleDisplayName: "Loofah Recording",
            CFBundleShortVersionString: version,
            CFBundleVersion: buildNumber,
            NSExtension: {
              NSExtensionPointIdentifier: "com.apple.widgetkit-extension",
            },
          },
        },
      },
    },
  };
  writeFileSync(
    resolve(apple, "live-activity.json"),
    `${JSON.stringify(spec, null, 2)}\n`,
  );
  run("xcodegen", ["generate", "--spec", "live-activity.json"], apple);
}

if (!["init", "dev", "build", "prepare"].includes(command)) {
  throw new Error(
    "Usage: node scripts/ios.mjs <init|dev|build|prepare> [Tauri options]",
  );
}
if (command === "init") run("pnpm", ["tauri", "ios", "init", ...args]);
prepare();
if (command === "dev" || command === "build") {
  run("pnpm", ["tauri", "ios", command, ...args]);
}
