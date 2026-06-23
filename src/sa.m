#include "sa.h"
#include <CommonCrypto/CommonDigest.h>
#include <errno.h>

#define SUDOERS_PATH     "/private/etc/sudoers.d/yabai"
#define SUDOERS_TMP_PATH "/private/etc/sudoers.d/yabai.tmp"

extern int csr_get_active_config(uint32_t *config);
#define CSR_ALLOW_UNRESTRICTED_FS 0x02
#define CSR_ALLOW_TASK_FOR_PID    0x04

extern char g_sa_socket_file[MAXLEN];

static char osax_base_dir[MAXLEN];
static char osax_contents_dir[MAXLEN];
static char osax_contents_macos_dir[MAXLEN];
static char osax_contents_res_dir[MAXLEN];
static char osax_info_plist[MAXLEN];
static char osax_payload_dir[MAXLEN];
static char osax_payload_contents_dir[MAXLEN];
static char osax_payload_contents_macos_dir[MAXLEN];
static char osax_payload_plist[MAXLEN];
static char osax_bin_payload[MAXLEN];
static char osax_bin_loader[MAXLEN];

static char sa_plist[] =
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
    "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n"
    "<plist version=\"1.0\">\n"
    "<dict>\n"
    "<key>CFBundleDevelopmentRegion</key>\n"
    "<string>en</string>\n"
    "<key>CFBundleExecutable</key>\n"
    "<string>loader</string>\n"
    "<key>CFBundleIdentifier</key>\n"
    "<string>com.asmvik.yabai-osax</string>\n"
    "<key>CFBundleInfoDictionaryVersion</key>\n"
    "<string>6.0</string>\n"
    "<key>CFBundleName</key>\n"
    "<string>yabai</string>\n"
    "<key>CFBundlePackageType</key>\n"
    "<string>osax</string>\n"
    "<key>CFBundleShortVersionString</key>\n"
    "<string>"OSAX_VERSION"</string>\n"
    "<key>CFBundleVersion</key>\n"
    "<string>"OSAX_VERSION"</string>\n"
    "<key>NSHumanReadableCopyright</key>\n"
    "<string>Copyright © 2019 Åsmund Vikane. All rights reserved.</string>\n"
    "<key>OSAXHandlers</key>\n"
    "<dict>\n"
    "</dict>\n"
    "</dict>\n"
    "</plist>";

static char sa_bundle_plist[] =
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
    "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n"
    "<plist version=\"1.0\">\n"
    "<dict>\n"
    "<key>CFBundleDevelopmentRegion</key>\n"
    "<string>en</string>\n"
    "<key>CFBundleExecutable</key>\n"
    "<string>payload</string>\n"
    "<key>CFBundleIdentifier</key>\n"
    "<string>com.asmvik.yabai-sa</string>\n"
    "<key>CFBundleInfoDictionaryVersion</key>\n"
    "<string>6.0</string>\n"
    "<key>CFBundleName</key>\n"
    "<string>payload</string>\n"
    "<key>CFBundlePackageType</key>\n"
    "<string>BNDL</string>\n"
    "<key>CFBundleShortVersionString</key>\n"
    "<string>"OSAX_VERSION"</string>\n"
    "<key>CFBundleVersion</key>\n"
    "<string>"OSAX_VERSION"</string>\n"
    "<key>NSHumanReadableCopyright</key>\n"
    "<string>Copyright © 2019 Åsmund Vikane. All rights reserved.</string>\n"
    "<key>NSPrincipalClass</key>\n"
    "<string></string>\n"
    "</dict>\n"
    "</plist>";

static void scripting_addition_set_path(void)
{
    snprintf(osax_base_dir, sizeof(osax_base_dir), "%s", "/Library/ScriptingAdditions/yabai.osax");

    snprintf(osax_contents_dir, sizeof(osax_contents_dir), "%s/%s", osax_base_dir, "Contents");
    snprintf(osax_contents_macos_dir, sizeof(osax_contents_macos_dir), "%s/%s", osax_contents_dir, "MacOS");
    snprintf(osax_contents_res_dir, sizeof(osax_contents_res_dir), "%s/%s", osax_contents_dir, "Resources");
    snprintf(osax_info_plist, sizeof(osax_info_plist), "%s/%s", osax_contents_dir, "Info.plist");

    snprintf(osax_payload_dir, sizeof(osax_payload_dir), "%s/%s", osax_contents_res_dir, "payload.bundle");
    snprintf(osax_payload_contents_dir, sizeof(osax_payload_contents_dir), "%s/%s", osax_payload_dir, "Contents");
    snprintf(osax_payload_contents_macos_dir, sizeof(osax_payload_contents_macos_dir), "%s/%s", osax_payload_contents_dir, "MacOS");
    snprintf(osax_payload_plist, sizeof(osax_payload_plist), "%s/%s", osax_payload_contents_dir, "Info.plist");

    snprintf(osax_bin_loader, sizeof(osax_bin_loader), "%s/%s", osax_contents_macos_dir, "loader");
    snprintf(osax_bin_payload, sizeof(osax_bin_payload), "%s/%s", osax_payload_contents_macos_dir, "payload");
}

static bool scripting_addition_create_directory(void)
{
    if (mkdir(osax_base_dir, 0755))                   goto err;
    if (mkdir(osax_contents_dir, 0755))               goto err;
    if (mkdir(osax_contents_macos_dir, 0755))         goto err;
    if (mkdir(osax_contents_res_dir, 0755))           goto err;
    if (mkdir(osax_payload_dir, 0755))                goto err;
    if (mkdir(osax_payload_contents_dir, 0755))       goto err;
    if (mkdir(osax_payload_contents_macos_dir, 0755)) goto err;
    return true;
err:
    return false;
}

static bool scripting_addition_write_file(char *buffer, unsigned int size, char *file, char *file_mode)
{
    FILE *handle = fopen(file, file_mode);
    if (!handle) return false;

    size_t bytes = fwrite(buffer, size, 1, handle);
    bool result = bytes == 1;
    fclose(handle);

    return result;
}

#ifdef __arm64__
#define MACHO_CPU_TYPE_ARM64       16777228
#define MACHO_CPU_SUBTYPE_ARM64E   2
#define MACHO_CPU_SUBTYPE_MASK     0x00FFFFFFu
#define MACHO_FAT_MAGIC            0xCAFEBABEu
#define MACHO_MH_MAGIC_64          0xFEEDFACFu
#define MACHO_DOCK_PATH            "/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock"

static bool macho_read_u32_be(FILE *handle, long offset, uint32_t *result)
{
    uint8_t bytes[4];
    if (fseek(handle, offset, SEEK_SET) != 0) return false;
    if (fread(bytes, sizeof(bytes), 1, handle) != 1) return false;

    *result = ((uint32_t) bytes[0] << 24) |
              ((uint32_t) bytes[1] << 16) |
              ((uint32_t) bytes[2] << 8)  |
              ((uint32_t) bytes[3]);
    return true;
}

static bool macho_read_u32_le(FILE *handle, long offset, uint32_t *result)
{
    uint8_t bytes[4];
    if (fseek(handle, offset, SEEK_SET) != 0) return false;
    if (fread(bytes, sizeof(bytes), 1, handle) != 1) return false;

    *result = ((uint32_t) bytes[3] << 24) |
              ((uint32_t) bytes[2] << 16) |
              ((uint32_t) bytes[1] << 8)  |
              ((uint32_t) bytes[0]);
    return true;
}

static bool macho_find_arm64e_caps(FILE *handle, uint8_t *caps, long *fat_caps_offset, long *mach_caps_offset)
{
    uint32_t magic;
    if (!macho_read_u32_be(handle, 0, &magic)) return false;

    *fat_caps_offset = -1;
    *mach_caps_offset = -1;

    // NOTE(plus): only 32-bit fat (FAT_MAGIC, 0xCAFEBABE) is handled, not
    // FAT_MAGIC_64 (0xCAFEBABF) with its 32-byte fat_arch_64 stride. Dock and the
    // yabai loader are both 32-bit fat today; revisit if that ever changes.
    if (magic == MACHO_FAT_MAGIC) {
        uint32_t arch_count;
        if (!macho_read_u32_be(handle, 4, &arch_count)) return false;

        for (uint32_t i = 0; i < arch_count; ++i) {
            long arch_offset = 8 + (long) i * 20;
            uint32_t cputype, cpusubtype, slice_offset;

            if (!macho_read_u32_be(handle, arch_offset, &cputype)) return false;
            if (!macho_read_u32_be(handle, arch_offset + 4, &cpusubtype)) return false;
            if (!macho_read_u32_be(handle, arch_offset + 8, &slice_offset)) return false;

            if (cputype != MACHO_CPU_TYPE_ARM64) continue;
            if ((cpusubtype & MACHO_CPU_SUBTYPE_MASK) != MACHO_CPU_SUBTYPE_ARM64E) continue;

            uint32_t slice_magic, slice_cputype, slice_cpusubtype;
            if (!macho_read_u32_le(handle, slice_offset, &slice_magic)) return false;
            if (slice_magic != MACHO_MH_MAGIC_64) return false;
            if (!macho_read_u32_le(handle, slice_offset + 4, &slice_cputype)) return false;
            if (!macho_read_u32_le(handle, slice_offset + 8, &slice_cpusubtype)) return false;
            if (slice_cputype != MACHO_CPU_TYPE_ARM64) return false;
            if ((slice_cpusubtype & MACHO_CPU_SUBTYPE_MASK) != MACHO_CPU_SUBTYPE_ARM64E) return false;

            *caps = cpusubtype >> 24;
            *fat_caps_offset = arch_offset + 4;
            *mach_caps_offset = slice_offset + 11;
            return true;
        }

        return false;
    }

    if (!macho_read_u32_le(handle, 0, &magic)) return false;
    if (magic == MACHO_MH_MAGIC_64) {
        uint32_t cputype, cpusubtype;
        if (!macho_read_u32_le(handle, 4, &cputype)) return false;
        if (!macho_read_u32_le(handle, 8, &cpusubtype)) return false;
        if (cputype != MACHO_CPU_TYPE_ARM64) return false;
        if ((cpusubtype & MACHO_CPU_SUBTYPE_MASK) != MACHO_CPU_SUBTYPE_ARM64E) return false;

        *caps = cpusubtype >> 24;
        *mach_caps_offset = 11;
        return true;
    }

    return false;
}

static bool scripting_addition_patch_loader_pac_abi(bool *patched)
{
    *patched = false;

    FILE *dock = fopen(MACHO_DOCK_PATH, "rb");
    if (!dock) return false;

    uint8_t dock_caps;
    long ignored_fat_caps_offset, ignored_mach_caps_offset;
    bool dock_result = macho_find_arm64e_caps(dock, &dock_caps, &ignored_fat_caps_offset, &ignored_mach_caps_offset);
    fclose(dock);
    if (!dock_result) return false;

    FILE *loader = fopen(osax_bin_loader, "r+b");
    if (!loader) return false;

    uint8_t loader_caps;
    long fat_caps_offset, mach_caps_offset;
    bool loader_result = macho_find_arm64e_caps(loader, &loader_caps, &fat_caps_offset, &mach_caps_offset);
    if (!loader_result) goto err;

    // NOTE(plus): the fat and mach-header capability bytes are independent on
    // disk, so decide "needs patch" from *both* (not just the fat byte the finder
    // returns) -- otherwise a prior partial write that left them out of sync would
    // be read as already-correct and never repaired. Then write the mach byte
    // first and the fat byte last: the fat byte is what macho_find_arm64e_caps
    // keys on for a fat binary, so writing it last means an interrupted patch
    // leaves the fat byte stale and is re-detected on the next run.
    bool needs_patch = false;

    if (fat_caps_offset != -1) {
        if (fseek(loader, fat_caps_offset, SEEK_SET) != 0) goto err;
        int byte = fgetc(loader);
        if (byte == EOF) goto err;
        if ((uint8_t) byte != dock_caps) needs_patch = true;
    }

    if (mach_caps_offset != -1) {
        if (fseek(loader, mach_caps_offset, SEEK_SET) != 0) goto err;
        int byte = fgetc(loader);
        if (byte == EOF) goto err;
        if ((uint8_t) byte != dock_caps) needs_patch = true;
    }

    if (!needs_patch) goto out;

    if (mach_caps_offset != -1) {
        if (fseek(loader, mach_caps_offset, SEEK_SET) != 0) goto err;
        if (fputc(dock_caps, loader) == EOF) goto err;
    }

    if (fat_caps_offset != -1) {
        if (fseek(loader, fat_caps_offset, SEEK_SET) != 0) goto err;
        if (fputc(dock_caps, loader) == EOF) goto err;
    }

    // A buffered-write error can surface only at close, so a failed fclose here
    // means the patch may not have hit disk -- report it instead of returning ok.
    if (fclose(loader) != 0) return false;
    *patched = true;
    return true;

out:
    fclose(loader);
    return true;

err:
    fclose(loader);
    return false;
}
#endif

static void scripting_addition_prepare_binaries(void)
{
    char cmd[MAXLEN];

    snprintf(cmd, sizeof(cmd), "%s %s", "chmod +x", osax_bin_loader);
    system(cmd);

#ifdef __arm64__
    bool loader_patched;
    if (!scripting_addition_patch_loader_pac_abi(&loader_patched)) {
        warn("yabai: scripting-addition failed to normalize loader arm64e PAC ABI!\n");
    }
    // NOTE(plus): codesign runs unconditionally below regardless of loader_patched,
    // since the loader bytes were just (re)written from the embedded payload anyway.
#endif

    snprintf(cmd, sizeof(cmd), "%s %s %s", "codesign -f -s -", osax_bin_loader, "2>/dev/null");
    system(cmd);

    snprintf(cmd, sizeof(cmd), "%s %s", "chmod +x", osax_bin_payload);
    system(cmd);

    snprintf(cmd, sizeof(cmd), "%s %s %s", "codesign -f -s -", osax_bin_payload, "2>/dev/null");
    system(cmd);
}

static void scripting_addition_restart_dock(void)
{
    NSArray *dock = [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.dock"];
    [dock makeObjectsPerformSelector:@selector(terminate)];
}

static bool scripting_addition_set_socket_path(void)
{
    const char *sudo_uid = getenv("SUDO_UID");

    uid_t uid = getuid();
    assert(uid == 0);

    if (sudo_uid == NULL)                  return false;
    if (sscanf(sudo_uid, "%u", &uid) != 1) return false;

    struct passwd *pw = getpwuid(uid);
    if (!pw) return false;

    snprintf(g_sa_socket_file, sizeof(g_sa_socket_file), SA_SOCKET_PATH_FMT, pw->pw_name);
    return true;
}

static bool scripting_addition_is_installed(void)
{
    if (osax_base_dir[0] == 0) scripting_addition_set_path();

    DIR *dir = opendir(osax_base_dir);
    if (!dir) return false;

    closedir(dir);
    return true;
}

static int scripting_addition_check(void)
{
    bool result = 0;
    NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];

    if (scripting_addition_is_installed()) {
        NSString *payload_path = [NSString stringWithUTF8String:osax_payload_dir];
        NSBundle *payload_bundle = [NSBundle bundleWithPath:payload_path];
        NSString *ns_version = [payload_bundle objectForInfoDictionaryKey:@"CFBundleVersion"];

        bool status = string_equals([ns_version UTF8String], OSAX_VERSION);
        result = status ? 0 : 1;
    } else {
        result = 1;
    }

    [pool drain];
    return result;
}

static bool scripting_addition_remove(void)
{
    char cmd[MAXLEN];
    snprintf(cmd, sizeof(cmd), "%s %s %s", "rm -rf", osax_base_dir, "2>/dev/null");

    int code = system(cmd);
    return code == 0;
}

static int scripting_addition_install(void)
{
    umask(S_IWGRP | S_IWOTH);

    if ((scripting_addition_is_installed()) && (!scripting_addition_remove())) {
        return 1;
    }

    if (!scripting_addition_create_directory()) {
        goto cleanup;
    }

    if (!scripting_addition_write_file(sa_plist, strlen(sa_plist), osax_info_plist, "w")) {
        goto cleanup;
    }

    if (!scripting_addition_write_file(sa_bundle_plist, strlen(sa_bundle_plist), osax_payload_plist, "w")) {
        goto cleanup;
    }

    if (!scripting_addition_write_file((char *) __src_osax_loader, __src_osax_loader_len, osax_bin_loader, "wb")) {
        goto cleanup;
    }

    if (!scripting_addition_write_file((char *) __src_osax_payload, __src_osax_payload_len, osax_bin_payload, "wb")) {
        goto cleanup;
    }

    scripting_addition_prepare_binaries();
    scripting_addition_restart_dock();
    return 0;

cleanup:
    scripting_addition_remove();
    return 2;
}

static bool scripting_addition_request_handshake(char *version, uint32_t *attrib)
{
    int sockfd;
    bool result = false;
    char rsp[BUFSIZ] = {0};
    char bytes[SA_SOCKET_BUFF_LEN] = { 0x01, 0x00, SA_OPCODE_HANDSHAKE };

    if (socket_open(&sockfd)) {
        if (socket_connect(sockfd, g_sa_socket_file)) {
            if (send(sockfd, bytes, 3, 0) != -1) {
                int length = recv(sockfd, rsp, sizeof(rsp)-1, 0);
                if (length <= 0) goto out;

                char *zero = rsp;
                while (*zero != '\0') ++zero;

                assert(*zero == '\0');
                memcpy(version, rsp, zero - rsp + 1);
                memcpy(attrib, zero+1, sizeof(uint32_t));

                result = true;
            }
        }

out:
        socket_close(sockfd);
    }

    return result;
}

static int scripting_addition_perform_validation(void)
{
    uint32_t attrib = 0;
    char version[SA_SOCKET_BUFF_LEN] = {0};
    bool is_latest_version_installed = scripting_addition_check() == 0;

    if (!scripting_addition_request_handshake(version, &attrib)) {
        notify("scripting-addition", "connection failed!");
        return 1;
    }

    if (string_equals(version, OSAX_VERSION)) {
        if ((attrib & OSAX_ATTRIB_ALL) == OSAX_ATTRIB_ALL) {
            notify("scripting-addition", "payload v%s", version);
            return 0;
        }

        notify("scripting-addition", "payload (0x%X) doesn't support this macOS version!", attrib);
        return 1;
    }

    if (!is_latest_version_installed) {
        notify("scripting-addition", "payload is outdated, updating..");
        return scripting_addition_install();
    }

    notify("scripting-addition", "payload is outdated, restarting Dock.app..");
    scripting_addition_restart_dock();
    return 0;
}

static bool scripting_addition_is_sip_friendly(void)
{
    uint32_t config = 0;
    csr_get_active_config(&config);

    if (!(config & CSR_ALLOW_UNRESTRICTED_FS)) {
        return false;
    }

    if (!(config & CSR_ALLOW_TASK_FOR_PID)) {
        return false;
    }

    return true;
}

#ifdef __arm64__
static bool scripting_addition_is_arm64e_enabled(void)
{
    char bootargs[2048];
    size_t len = sizeof(bootargs) - 1;

    if (sysctlbyname("kern.bootargs", bootargs, &len, NULL, 0) == 0) {
        if (strnstr(bootargs, "-arm64e_preview_abi", len)) {
            return true;
        }
    }

    return false;
}
#endif

static bool mach_loader_inject_payload(void)
{
    FILE *handle = popen("/Library/ScriptingAdditions/yabai.osax/Contents/MacOS/loader", "r");
    if (!handle) return false;

    int result = pclose(handle);
    if (WIFEXITED(result)) {
        return WEXITSTATUS(result) == 0;
    } else if (WIFSIGNALED(result)) {
        return false;
    } else if (WIFSTOPPED(result)) {
        return false;
    }

    return false;
}

int scripting_addition_uninstall(void)
{
    if (!scripting_addition_is_sip_friendly()) {
        warn("yabai: System Integrity Protection: Filesystem Protections and Debugging Restrictions must be disabled!\n");
        notify("scripting-addition", "System Integrity Protection: Filesystem Protections and Debugging Restrictions must be disabled!");
        return 1;
    }

    if (!is_root()) {
        warn("yabai: scripting-addition must be uninstalled as root!\n");
        notify("scripting-addition", "must be uninstalled as root!");
        return 1;
    }

    if (!scripting_addition_is_installed()) return  0;
    if (!scripting_addition_remove())       return -1;
    return 0;
}

int scripting_addition_load(void)
{
    int result = 0;
    NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];

    if (!is_root()) {
        warn("yabai: scripting-addition must be loaded as root!\n");
        notify("scripting-addition", "must be loaded as root!");
        result = 1;
        goto out;
    }

    if (!scripting_addition_is_sip_friendly()) {
        warn("yabai: System Integrity Protection: Filesystem Protections and Debugging Restrictions must be disabled!\n");
        notify("scripting-addition", "System Integrity Protection: Filesystem Protections and Debugging Restrictions must be disabled!");
        result = 1;
        goto out;
    }

    if (scripting_addition_check() != 0) {
        result = scripting_addition_install();
        goto out;
    }

#ifdef __arm64__
    if (!scripting_addition_is_arm64e_enabled()) {
        warn("yabai: missing required nvram boot-arg '-arm64e_preview_abi'!\n");
        notify("scripting-addition", "missing required nvram boot-arg '-arm64e_preview_abi'!");
        result = 1;
        goto out;
    }

    bool loader_patched;
    if (!scripting_addition_patch_loader_pac_abi(&loader_patched)) {
        warn("yabai: scripting-addition failed to check loader arm64e PAC ABI!\n");
    } else if (loader_patched) {
        char cmd[MAXLEN];
        snprintf(cmd, sizeof(cmd), "%s %s %s", "codesign -f -s -", osax_bin_loader, "2>/dev/null");
        system(cmd);
    }
#endif

    if (!mach_loader_inject_payload()) {
        warn("yabai: scripting-addition failed to inject payload into Dock.app!\n");
        notify("scripting-addition", "failed to inject payload into Dock.app!");
        result = 1;
        goto out;
    }

    if (scripting_addition_set_socket_path()) {
        result = scripting_addition_perform_validation();
    }

out:
    [pool drain];
    return result;
}

//
// NOTE(plus): Report whether the scripting addition is actually live, without root and
// without re-injecting. We talk to the payload's socket directly (handshake opcode), so
// this reflects the real running state inside Dock -- not just whether files are installed.
// Exit 0 only when the payload answers, matches this build's OSAX_VERSION, and advertises
// full support for the current macOS. Handy because a missing/outdated SA silently sends
// window moves down the slow, blocking AX path.
//
int scripting_addition_status(void)
{
    char *user = getenv("USER");
    if (!user) {
        fprintf(stdout, "scripting-addition: cannot check -- 'env USER' not set\n");
        return 1;
    }
    snprintf(g_sa_socket_file, sizeof(g_sa_socket_file), SA_SOCKET_PATH_FMT, user);

    uint32_t attrib = 0;
    char version[SA_SOCKET_BUFF_LEN] = {0};

    if (!scripting_addition_request_handshake(version, &attrib)) {
        fprintf(stdout, "scripting-addition: NOT loaded (no response on %s)\n", g_sa_socket_file);
        return 1;
    }

    if (!string_equals(version, OSAX_VERSION)) {
        fprintf(stdout, "scripting-addition: loaded but OUTDATED (payload v%s, this build expects v%s) -- run 'sudo yabai --load-sa'\n", version, OSAX_VERSION);
        return 1;
    }

    if ((attrib & OSAX_ATTRIB_ALL) != OSAX_ATTRIB_ALL) {
        fprintf(stdout, "scripting-addition: loaded v%s but missing support for this macOS (attrib 0x%X)\n", version, attrib);
        return 1;
    }

    fprintf(stdout, "scripting-addition: loaded and healthy (payload v%s)\n", version);
    return 0;
}

//
// NOTE(plus): Manage a passwordless-sudo rule for `yabai --load-sa` so the launchd
// service -- which has no tty to type a password at -- can inject the scripting
// addition unattended. Without it, window moves fall back to the blocking AX path
// (the mid-drag freeze). This is the committed-binary equivalent of the dev-loop
// rule that `make dev` regenerates; see docs/debugging.md.
//
// The rule is pinned to the running binary's sha256, so it only authorizes *this*
// exact yabai (it stops authorizing the moment the binary changes) and only the
// `--load-sa` subcommand. We hash the binary in-process, write the rule to a temp
// file, validate it with `visudo -cf` *before* moving it into place (a malformed
// line can never lock sudo), and chmod it 0440 as sudo requires.
//
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
static bool sudoers_compute_sha256_hex(const char *path, char *out, size_t out_size)
{
    if (out_size < CC_SHA256_DIGEST_LENGTH*2 + 1) return false;

    FILE *handle = fopen(path, "rb");
    if (!handle) return false;

    CC_SHA256_CTX ctx;
    CC_SHA256_Init(&ctx);

    uint8_t buffer[65536];
    size_t bytes;
    while ((bytes = fread(buffer, 1, sizeof(buffer), handle)) > 0) {
        CC_SHA256_Update(&ctx, buffer, (CC_LONG) bytes);
    }

    bool read_ok = ferror(handle) == 0;
    fclose(handle);
    if (!read_ok) return false;

    uint8_t digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256_Final(digest, &ctx);

    for (int i = 0; i < CC_SHA256_DIGEST_LENGTH; ++i) {
        snprintf(out + i*2, 3, "%02x", digest[i]);
    }
    return true;
}
#pragma clang diagnostic pop

int scripting_addition_install_sudoers(void)
{
    if (!is_root()) {
        warn("yabai: sudoers rule must be installed as root! run 'sudo yabai --install-sudoers'\n");
        return 1;
    }

    //
    // NOTE(plus): we are root here, so getenv("USER") would read "root". The rule's
    // first field must name the *invoking* user, which sudo exposes as SUDO_USER.
    //
    char *user = getenv("SUDO_USER");
    if (!user || !user[0]) {
        warn("yabai: cannot determine invoking user (env SUDO_USER not set); run via 'sudo yabai --install-sudoers'!\n");
        return 1;
    }

    //
    // NOTE(plus): pin the path sudo just resolved this invocation to -- that is the
    // same path a later `sudo yabai --load-sa` resolves, and the same one the launchd
    // service is launched with (both use _NSGetExecutablePath; see misc/service.h).
    //
    char exe_path[MAXLEN];
    unsigned int exe_path_size = sizeof(exe_path);
    if (_NSGetExecutablePath(exe_path, &exe_path_size) < 0) {
        warn("yabai: unable to retrieve path of executable!\n");
        return 1;
    }

    char sha[CC_SHA256_DIGEST_LENGTH*2 + 1];
    if (!sudoers_compute_sha256_hex(exe_path, sha, sizeof(sha))) {
        warn("yabai: unable to compute sha256 of '%s'!\n", exe_path);
        return 1;
    }

    char rule[MAXLEN];
    snprintf(rule, sizeof(rule), "%s ALL=(root) NOPASSWD: sha256:%s %s --load-sa\n", user, sha, exe_path);

    if (!scripting_addition_write_file(rule, strlen(rule), SUDOERS_TMP_PATH, "w")) {
        warn("yabai: failed to write '%s'!\n", SUDOERS_TMP_PATH);
        return 1;
    }

    if (chmod(SUDOERS_TMP_PATH, 0440) != 0) {
        warn("yabai: failed to set permissions on '%s'!\n", SUDOERS_TMP_PATH);
        unlink(SUDOERS_TMP_PATH);
        return 1;
    }

    char cmd[MAXLEN];
    snprintf(cmd, sizeof(cmd), "%s %s %s", "visudo -cf", SUDOERS_TMP_PATH, ">/dev/null 2>&1");
    int code = system(cmd);
    if (code == -1 || !WIFEXITED(code) || WEXITSTATUS(code) != 0) {
        warn("yabai: generated sudoers rule failed 'visudo -c' validation; not installing!\n");
        unlink(SUDOERS_TMP_PATH);
        return 1;
    }

    if (rename(SUDOERS_TMP_PATH, SUDOERS_PATH) != 0) {
        warn("yabai: failed to move sudoers rule into place at '%s'!\n", SUDOERS_PATH);
        unlink(SUDOERS_TMP_PATH);
        return 1;
    }

    fprintf(stdout, "yabai: installed passwordless '--load-sa' sudoers rule at '%s' for user '%s'\n", SUDOERS_PATH, user);
    return 0;
}

int scripting_addition_uninstall_sudoers(void)
{
    if (!is_root()) {
        warn("yabai: sudoers rule must be uninstalled as root! run 'sudo yabai --uninstall-sudoers'\n");
        return 1;
    }

    if (unlink(SUDOERS_PATH) != 0) {
        if (errno == ENOENT) {
            fprintf(stdout, "yabai: no sudoers rule installed at '%s'\n", SUDOERS_PATH);
            return 0;
        }
        warn("yabai: failed to remove '%s'!\n", SUDOERS_PATH);
        return 1;
    }

    fprintf(stdout, "yabai: removed sudoers rule at '%s'\n", SUDOERS_PATH);
    return 0;
}

#define sa_payload_init() char bytes[SA_SOCKET_BUFF_LEN]; int16_t length = 1+sizeof(length)
#define pack(v) memcpy(bytes+length, &v, sizeof(v)); length += sizeof(v)
#define sa_payload_send(op) *(int16_t*)bytes = length-sizeof(length), bytes[sizeof(length)] = op, scripting_addition_send_bytes(bytes, length)

static bool scripting_addition_send_bytes(char *bytes, int length)
{
    int sockfd;
    char dummy;
    bool result = false;

    if (socket_open(&sockfd)) {
        if (socket_connect(sockfd, g_sa_socket_file)) {
            if (send(sockfd, bytes, length, 0) != -1) {
                recv(sockfd, &dummy, 1, 0);
                result = true;
            }
        }

        socket_close(sockfd);
    }

    return result;
}

bool scripting_addition_focus_space(uint64_t sid)
{
    sa_payload_init();
    pack(sid);
    return sa_payload_send(SA_OPCODE_SPACE_FOCUS);
}

bool scripting_addition_create_space(uint64_t sid)
{
    sa_payload_init();
    pack(sid);
    return sa_payload_send(SA_OPCODE_SPACE_CREATE);
}

bool scripting_addition_destroy_space(uint64_t sid)
{
    sa_payload_init();
    pack(sid);
    return sa_payload_send(SA_OPCODE_SPACE_DESTROY);
}

bool scripting_addition_move_space_to_display(uint64_t src_sid, uint64_t dst_sid, uint64_t src_prev_sid, bool focus)
{
    sa_payload_init();
    pack(src_sid);
    pack(dst_sid);
    pack(src_prev_sid);
    pack(focus);
    return sa_payload_send(SA_OPCODE_SPACE_MOVE);
}

bool scripting_addition_move_space_after_space(uint64_t src_sid, uint64_t dst_sid, bool focus)
{
    uint64_t dummy_sid = 0;
    sa_payload_init();
    pack(src_sid);
    pack(dst_sid);
    pack(dummy_sid);
    pack(focus);
    return sa_payload_send(SA_OPCODE_SPACE_MOVE);
}

bool scripting_addition_move_window(uint32_t wid, int x, int y)
{
    sa_payload_init();
    pack(wid);
    pack(x);
    pack(y);
    return sa_payload_send(SA_OPCODE_WINDOW_MOVE);
}

bool scripting_addition_set_opacity(uint32_t wid, float opacity, float duration)
{
    sa_payload_init();
    pack(wid);
    pack(opacity);
    pack(duration);
    return sa_payload_send(duration > 0.0f ? SA_OPCODE_WINDOW_OPACITY_FADE : SA_OPCODE_WINDOW_OPACITY);
}

bool scripting_addition_set_layer(uint32_t wid, int layer)
{
    sa_payload_init();
    pack(wid);
    pack(layer);
    return sa_payload_send(SA_OPCODE_WINDOW_LAYER);
}

bool scripting_addition_set_sticky(uint32_t wid, bool sticky)
{
    sa_payload_init();
    pack(wid);
    pack(sticky);
    return sa_payload_send(SA_OPCODE_WINDOW_STICKY);
}

bool scripting_addition_set_shadow(uint32_t wid, bool shadow)
{
    sa_payload_init();
    pack(wid);
    pack(shadow);
    return sa_payload_send(SA_OPCODE_WINDOW_SHADOW);
}

bool scripting_addition_focus_window(uint32_t wid)
{
    sa_payload_init();
    pack(wid);
    return sa_payload_send(SA_OPCODE_WINDOW_FOCUS);
}

bool scripting_addition_scale_window(uint32_t wid, float x, float y, float w, float h)
{
    sa_payload_init();
    pack(wid);
    pack(x);
    pack(y);
    pack(w);
    pack(h);
    return sa_payload_send(SA_OPCODE_WINDOW_SCALE);
}

bool scripting_addition_swap_window_proxy_in(struct window_animation *animation_list, int animation_count)
{
    uint32_t dummy_wid = 0;
    sa_payload_init();
    pack(animation_count);
    for (int i = 0; i < animation_count; ++i) {
        if (__atomic_load_n(&animation_list[i].skip, __ATOMIC_RELAXED)) {
            pack(dummy_wid);
        } else {
            pack(animation_list[i].wid);
            pack(animation_list[i].proxy.id);
        }
    }
    return sa_payload_send(SA_OPCODE_WINDOW_SWAP_PROXY_IN);
}

bool scripting_addition_swap_window_proxy_out(struct window_animation *animation_list, int animation_count)
{
    uint32_t dummy_wid = 0;
    sa_payload_init();
    pack(animation_count);
    for (int i = 0; i < animation_count; ++i) {
        if (__atomic_load_n(&animation_list[i].skip, __ATOMIC_RELAXED)) {
            pack(dummy_wid);
        } else {
            pack(animation_list[i].wid);
            pack(animation_list[i].proxy.id);
        }
    }
    return sa_payload_send(SA_OPCODE_WINDOW_SWAP_PROXY_OUT);
}

bool scripting_addition_order_window(uint32_t a_wid, int order, uint32_t b_wid)
{
    sa_payload_init();
    pack(a_wid);
    pack(order);
    pack(b_wid);
    return sa_payload_send(SA_OPCODE_WINDOW_ORDER);
}

extern int g_connection;
bool scripting_addition_order_window_in(uint32_t *window_list, int window_count)
{
    uint32_t dummy_wid = 0;
    uint8_t ordered_in = 0;

    sa_payload_init();
    pack(window_count);
    for (int i = 0; i < window_count; ++i) {
        SLSWindowIsOrderedIn(g_connection, window_list[i], &ordered_in);
        if (ordered_in) {
            pack(dummy_wid);
        } else {
            pack(window_list[i]);
        }
    }
    return sa_payload_send(SA_OPCODE_WINDOW_ORDER_IN);
}

bool scripting_addition_move_window_list_to_space(uint64_t sid, uint32_t *window_list, int window_count)
{
    sa_payload_init();
    pack(sid);
    pack(window_count);
    for (int i = 0; i < window_count; ++i) {
        pack(window_list[i]);
    }
    return sa_payload_send(SA_OPCODE_WINDOW_LIST_TO_SPACE);
}

bool scripting_addition_move_window_to_space(uint64_t sid, uint32_t wid)
{
    sa_payload_init();
    pack(sid);
    pack(wid);
    return sa_payload_send(SA_OPCODE_WINDOW_TO_SPACE);
}

#undef sa_payload_init
#undef pack
#undef sa_payload_send
