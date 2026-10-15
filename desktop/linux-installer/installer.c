/* Offline per-user setup. The archive builder pins both payload hashes in this ELF. */
#include <gtk/gtk.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include "build-config.h"

#define APP_ID "in.krutrimmulyankan.evaluator"
#define OWNER_MAGIC "Krutrim Mulyankan graphical setup v1\n"
#define DESKTOP_MARK "X-Krutrim-Mulyankan-Managed=true\n"

typedef struct {
    char *self, *payload, *payload_icon, *data, *dir, *app, *manager;
    char *identity, *desktop, *icon;
} Paths;

static Paths paths;
static GtkWidget *window, *status_label, *install_button, *open_button, *remove_button, *close_button;
static gboolean busy, manage_only;

static gboolean fail(GError **error, const char *message) {
    g_set_error_literal(error, G_FILE_ERROR, G_FILE_ERROR_FAILED, message);
    return FALSE;
}

static gboolean system_error(GError **error, const char *action, const char *path) {
    g_set_error(error, G_FILE_ERROR, g_file_error_from_errno(errno),
                "%s: %s (%s)", action, path, g_strerror(errno));
    return FALSE;
}

/* Check every existing ancestor, including the final path. Never follow a link. */
static gboolean safe_path(const char *path, gboolean final_regular, GError **error) {
    if (!g_path_is_absolute(path) || strcmp(path, "/") == 0)
        return fail(error, "The installation location must be an absolute user folder.");
    for (const char *p = path; *p; ++p)
        if ((unsigned char)*p < 32 || *p == 127)
            return fail(error, "The installation location contains an unsupported control character.");
    char *walk = g_strdup(path);
    for (char *p = walk + 1; ; ++p) {
        if (*p && *p != '/') continue;
        gboolean last = *p == '\0';
        char saved = *p;
        *p = '\0';
        struct stat st;
        if (lstat(walk, &st) == 0) {
            if (S_ISLNK(st.st_mode)) {
                g_free(walk);
                return fail(error, "Setup will not write through a symbolic link. Choose a normal user data folder.");
            }
            if ((!last || !final_regular) && !S_ISDIR(st.st_mode)) {
                g_free(walk);
                return fail(error, "An installation folder is occupied by another file.");
            }
            if (last && final_regular && (!S_ISREG(st.st_mode) || st.st_uid != getuid() || st.st_nlink != 1)) {
                g_free(walk);
                return fail(error, "An installation file is not a regular file owned by your account.");
            }
        } else if (errno != ENOENT) {
            gboolean result = system_error(error, "Could not inspect the installation location", walk);
            g_free(walk);
            return result;
        }
        *p = saved;
        if (last) break;
    }
    g_free(walk);
    return TRUE;
}

static gboolean make_parent(const char *file, GError **error) {
    char *parent = g_path_get_dirname(file);
    gboolean result = safe_path(file, TRUE, error);
    if (result && g_mkdir_with_parents(parent, 0755) != 0)
        result = system_error(error, "Could not create the installation folder", parent);
    if (result) result = safe_path(file, TRUE, error);
    g_free(parent);
    return result;
}

static gboolean init_paths(GError **error) {
    char self[PATH_MAX + 1];
    ssize_t size = readlink("/proc/self/exe", self, PATH_MAX);
    if (size < 0) return system_error(error, "Could not locate setup", "/proc/self/exe");
    self[size] = '\0';
    paths.self = g_strdup(self);
    char *base = g_path_get_dirname(self);
    paths.payload = g_build_filename(base, "payload", "application.AppImage", NULL);
    paths.payload_icon = g_build_filename(base, "payload", "icon.png", NULL);
    g_free(base);
    const char *home = g_getenv("HOME");
    if (!home || !g_path_is_absolute(home)) return fail(error, "Your home folder could not be found.");
    const char *data = g_getenv("XDG_DATA_HOME");
    paths.data = data && g_path_is_absolute(data) ? g_canonicalize_filename(data, NULL)
        : g_build_filename(home, ".local", "share", NULL);
    paths.dir = g_build_filename(paths.data, "krutrim-mulyankan", NULL);
    paths.app = g_build_filename(paths.dir, "application.AppImage", NULL);
    paths.manager = g_build_filename(paths.dir, "installer", NULL);
    paths.identity = g_build_filename(paths.dir, "setup-owner", NULL);
    paths.desktop = g_build_filename(paths.data, "applications", APP_ID ".desktop", NULL);
    paths.icon = g_build_filename(paths.data, "icons", "hicolor", "256x256", "apps", APP_ID ".png", NULL);
    return safe_path(paths.data, FALSE, error);
}

static char *read_regular(const char *path) {
    int fd = open(path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) return NULL;
    struct stat st;
    if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_uid != getuid() || st.st_size > 16384) {
        close(fd);
        return NULL;
    }
    char *text = g_malloc0((gsize)st.st_size + 1);
    ssize_t count = read(fd, text, (size_t)st.st_size);
    close(fd);
    if (count != st.st_size) g_clear_pointer(&text, g_free);
    return text;
}

static gboolean owned_install(void) {
    char *identity = read_regular(paths.identity);
    gboolean owned = identity && g_str_has_prefix(identity, OWNER_MAGIC);
    g_free(identity);
    return owned;
}

static gboolean write_all(int fd, const void *data, size_t length, GError **error) {
    const char *position = data;
    while (length) {
        ssize_t count = write(fd, position, length);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) return system_error(error, "Could not write setup files", "temporary file");
        position += count;
        length -= (size_t)count;
    }
    return TRUE;
}

static int temporary_file(const char *destination, char **temporary, GError **error) {
    if (!make_parent(destination, error)) return -1;
    *temporary = g_strconcat(destination, ".tmp-XXXXXX", NULL);
    int fd = g_mkstemp_full(*temporary, O_RDWR | O_CLOEXEC, 0600);
    if (fd < 0) system_error(error, "Could not prepare setup files", destination);
    return fd;
}

static gboolean finish_file(int fd, mode_t mode, GError **error) {
    gboolean result = TRUE;
    if (fchmod(fd, mode) != 0 || fsync(fd) != 0)
        result = system_error(error, "Could not finish setup files", "temporary file");
    if (close(fd) != 0 && result)
        result = system_error(error, "Could not close setup files", "temporary file");
    return result;
}

static gboolean stage_copy(const char *source, const char *destination, const char *expected,
                           mode_t mode, char **temporary, GError **error) {
    int input = open(source, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (input < 0) return system_error(error, "Could not open the offline installation file", source);
    struct stat st;
    if (fstat(input, &st) != 0 || !S_ISREG(st.st_mode)) {
        close(input);
        return fail(error, "The offline installation file is not a regular file.");
    }
    int output = temporary_file(destination, temporary, error);
    if (output < 0) { close(input); return FALSE; }
    GChecksum *checksum = g_checksum_new(G_CHECKSUM_SHA256);
    gboolean result = TRUE;
    unsigned char buffer[65536];
    for (;;) {
        ssize_t count = read(input, buffer, sizeof(buffer));
        if (count < 0 && errno == EINTR) continue;
        if (count < 0) { result = system_error(error, "Could not read the offline installation file", source); break; }
        if (!count) break;
        g_checksum_update(checksum, buffer, (gsize)count);
        if (!write_all(output, buffer, (size_t)count, error)) { result = FALSE; break; }
    }
    close(input);
    if (result && expected && strcmp(expected, g_checksum_get_string(checksum)) != 0)
        result = fail(error, "The offline installation file failed its integrity check. Download setup again from krutrimmulyankan.in.");
    g_checksum_free(checksum);
    if (result) result = finish_file(output, mode, error);
    else close(output);
    return result;
}

static gboolean stage_text(const char *text, const char *destination, char **temporary, GError **error) {
    int fd = temporary_file(destination, temporary, error);
    if (fd < 0) return FALSE;
    if (!write_all(fd, text, strlen(text), error)) { close(fd); return FALSE; }
    return finish_file(fd, 0644, error);
}

/* Desktop Exec has its own quoting rules; it is never evaluated by a shell. */
static char *exec_quote(const char *path) {
    GString *quoted = g_string_new("\"");
    for (const char *p = path; *p; ++p) {
        if (*p == '%') g_string_append_c(quoted, '%');
        if (*p == '\\' || *p == '"' || *p == '`' || *p == '$') g_string_append_c(quoted, '\\');
        g_string_append_c(quoted, *p);
    }
    g_string_append_c(quoted, '"');
    return g_string_free(quoted, FALSE);
}

static char *desktop_string(const char *path) {
    GString *value = g_string_new(NULL);
    for (const char *p = path; *p; ++p) {
        if (*p == '\\') g_string_append_c(value, '\\');
        g_string_append_c(value, *p);
    }
    return g_string_free(value, FALSE);
}

static gboolean validate_destinations(GError **error) {
    const char *files[] = {paths.app, paths.manager, paths.identity, paths.desktop, paths.icon};
    for (guint i = 0; i < G_N_ELEMENTS(files); ++i)
        if (!safe_path(files[i], TRUE, error)) return FALSE;
    return TRUE;
}

/* Retain the prior inode until all staged files have been activated. */
static gboolean backup_file(const char *destination, char **backup, GError **error) {
    struct stat st;
    if (lstat(destination, &st) != 0) {
        if (errno == ENOENT) return TRUE;
        return system_error(error, "Could not inspect the previous installation", destination);
    }
    *backup = g_strconcat(destination, ".previous-XXXXXX", NULL);
    int fd = g_mkstemp_full(*backup, O_RDWR | O_CLOEXEC, 0600);
    if (fd < 0) return system_error(error, "Could not preserve the previous installation", destination);
    close(fd);
    if (unlink(*backup) != 0 || link(destination, *backup) != 0)
        return system_error(error, "Could not preserve the previous installation", destination);
    return TRUE;
}

static gboolean activate_files(const char **destinations, char **staged, guint count, GError **error) {
    char **backups = g_new0(char *, count);
    gboolean *activated = g_new0(gboolean, count);
    gboolean result = TRUE;
    for (guint i = 0; result && i < count; ++i)
        result = backup_file(destinations[i], &backups[i], error);
    /* The AppImage is last so its old inode remains available through activation. */
    const guint order[] = {2, 1, 3, 4, 0};
    for (guint i = 0; result && i < count; ++i) {
        guint index = order[i];
        if (rename(staged[index], destinations[index]) == 0) activated[index] = TRUE;
        else result = system_error(error, "Could not activate the installation", destinations[index]);
    }
    if (!result) {
        for (guint i = count; i > 0; --i) {
            guint index = order[i - 1];
            if (!activated[index]) continue;
            if (backups[index]) {
                if (rename(backups[index], destinations[index]) != 0) {
                    /* Leave the old inode recoverable if the filesystem also refuses rollback. */
                    g_prefix_error(error, "The previous file is preserved at %s. ", backups[index]);
                    g_clear_pointer(&backups[index], g_free);
                }
            } else if (unlink(destinations[index]) != 0 && errno != ENOENT) {
                g_prefix_error(error, "A partial setup file remains at %s. ", destinations[index]);
            }
        }
    }
    for (guint i = 0; i < count; ++i) {
        if (backups[i]) { unlink(backups[i]); g_free(backups[i]); }
    }
    g_free(backups); g_free(activated);
    return result;
}

static gboolean install_app(GError **error) {
    if (getuid() == 0 || geteuid() == 0) return fail(error, "Open setup using your regular account. An administrator password is not needed.");
    if (!validate_destinations(error)) return FALSE;
    if (!owned_install() && (g_file_test(paths.app, G_FILE_TEST_EXISTS) ||
        g_file_test(paths.identity, G_FILE_TEST_EXISTS) || g_file_test(paths.manager, G_FILE_TEST_EXISTS)))
        return fail(error, "The installation folder contains files from another installation. Setup has left them unchanged.");
    char *existing_desktop = read_regular(paths.desktop);
    if (g_file_test(paths.desktop, G_FILE_TEST_EXISTS) &&
        (!existing_desktop || !strstr(existing_desktop, DESKTOP_MARK))) {
        g_free(existing_desktop);
        return fail(error, "An existing application-menu entry is not owned by this setup. It has been left unchanged.");
    }
    g_free(existing_desktop);
    if (!owned_install() && g_file_test(paths.icon, G_FILE_TEST_EXISTS))
        return fail(error, "An existing icon is not owned by this setup. It has been left unchanged.");
    char *app_arg = exec_quote(paths.app), *manager_arg = exec_quote(paths.manager);
    char *icon_value = desktop_string(paths.icon);
    /* String values are unescaped before Exec is parsed; escape backslashes twice. */
    char *app_exec = desktop_string(app_arg), *manager_exec = desktop_string(manager_arg);
    char *desktop = g_strdup_printf(
        "[Desktop Entry]\nType=Application\nName=Krutrim Mulyankan\n"
        "Comment=Check AI models on your own computer\nExec=/usr/bin/env -- %s --appimage-extract-and-run\n"
        "Icon=%s\nTerminal=false\nCategories=Utility;\nStartupNotify=true\n"
        "Actions=Manage;\n" DESKTOP_MARK "\n[Desktop Action Manage]\n"
        "Name=Manage installation\nExec=/usr/bin/env -- %s --manage\n", app_exec, icon_value, manager_exec);
    char *identity = g_strdup(OWNER_MAGIC "version=" KM_VERSION "\nicon-sha256=" KM_ICON_SHA256 "\n");
    const char *destinations[] = {paths.app, paths.manager, paths.icon, paths.desktop, paths.identity};
    char *staged[G_N_ELEMENTS(destinations)] = {NULL};
    gboolean result = stage_copy(paths.payload, paths.app, KM_APP_SHA256, 0755, &staged[0], error)
        && stage_copy(paths.self, paths.manager, NULL, 0755, &staged[1], error)
        && stage_copy(paths.payload_icon, paths.icon, KM_ICON_SHA256, 0644, &staged[2], error)
        && stage_text(desktop, paths.desktop, &staged[3], error)
        && stage_text(identity, paths.identity, &staged[4], error);
    /* No existing files are touched until every payload has been copied and checked. */
    if (result) result = validate_destinations(error);
    if (result) result = activate_files(destinations, staged, G_N_ELEMENTS(destinations), error);
    for (guint i = 0; i < G_N_ELEMENTS(staged); ++i) {
        if (staged[i]) { unlink(staged[i]); g_free(staged[i]); }
    }
    g_free(app_arg); g_free(manager_arg); g_free(app_exec); g_free(manager_exec);
    g_free(icon_value); g_free(desktop); g_free(identity);
    return result;
}

static gboolean open_app(GError **error) {
    if (!owned_install()) return fail(error, "Install Krutrim Mulyankan before opening it.");
    if (!safe_path(paths.app, TRUE, error)) return FALSE;
    char *argv[] = {paths.app, "--appimage-extract-and-run", NULL};
    return g_spawn_async(NULL, argv, NULL, G_SPAWN_STDOUT_TO_DEV_NULL | G_SPAWN_STDERR_TO_DEV_NULL,
                         NULL, NULL, NULL, error);
}

static gboolean unlink_file(const char *path, GError **error) {
    if (unlink(path) == 0 || errno == ENOENT) return TRUE;
    return system_error(error, "Could not remove the installed app file", path);
}

static gboolean remove_app(GError **error) {
    if (getuid() == 0 || geteuid() == 0) return fail(error, "Remove this app using the account that installed it.");
    if (!validate_destinations(error)) return FALSE;
    if (!owned_install()) return fail(error, "This setup does not own the installed app. No files were removed.");
    char *desktop = read_regular(paths.desktop);
    if (desktop && strstr(desktop, DESKTOP_MARK) && !unlink_file(paths.desktop, error)) {
        g_free(desktop); return FALSE;
    }
    g_free(desktop);
    /* Only a matching managed icon is removed; user replacements are preserved. */
    char *identity = read_regular(paths.identity);
    char *icon = NULL;
    gsize icon_size = 0;
    if (g_file_get_contents(paths.icon, &icon, &icon_size, NULL)) {
        char *digest = g_compute_checksum_for_data(G_CHECKSUM_SHA256, (guchar *)icon, icon_size);
        char *record = g_strconcat("icon-sha256=", digest, "\n", NULL);
        if (identity && strstr(identity, record) && !unlink_file(paths.icon, error)) {
            g_free(identity); g_free(icon); g_free(digest); g_free(record); return FALSE;
        }
        g_free(digest); g_free(record); g_free(icon);
    }
    g_free(identity);
    if (!unlink_file(paths.app, error) || !unlink_file(paths.manager, error) || !unlink_file(paths.identity, error))
        return FALSE;
    /* Never recursively remove this folder: results or other user files may exist. */
    rmdir(paths.dir);
    return TRUE;
}

static void refresh_controls(const char *message) {
    gboolean installed = owned_install() && g_file_test(paths.app, G_FILE_TEST_IS_REGULAR);
    gboolean payload = g_file_test(paths.payload, G_FILE_TEST_IS_REGULAR);
    gtk_label_set_text(GTK_LABEL(status_label), message ? message : installed
        ? "Krutrim Mulyankan is installed for your account."
        : "Install the app for your account. No administrator password is needed.");
    gtk_widget_set_visible(install_button, payload && !manage_only);
    gtk_button_set_label(GTK_BUTTON(install_button), installed ? "Reinstall & Open" : "Install & Open");
    gtk_widget_set_visible(open_button, installed);
    gtk_widget_set_visible(remove_button, installed);
    gtk_widget_set_sensitive(install_button, !busy);
    gtk_widget_set_sensitive(open_button, !busy);
    gtk_widget_set_sensitive(remove_button, !busy);
    gtk_widget_set_sensitive(close_button, !busy);
}

static void show_error(GError *error) {
    GtkWidget *dialog = gtk_message_dialog_new(GTK_WINDOW(window), GTK_DIALOG_MODAL,
        GTK_MESSAGE_ERROR, GTK_BUTTONS_CLOSE, "Could not complete setup");
    gtk_message_dialog_format_secondary_text(GTK_MESSAGE_DIALOG(dialog), "%s", error->message);
    gtk_dialog_run(GTK_DIALOG(dialog));
    gtk_widget_destroy(dialog);
    g_error_free(error);
}

static void install_clicked(GtkButton *button, gpointer user_data) {
    busy = TRUE;
    refresh_controls("Installing the app and adding its application-menu icon…");
    while (gtk_events_pending()) gtk_main_iteration();
    GError *error = NULL;
    gboolean installed = install_app(&error);
    if (installed) installed = open_app(&error);
    busy = FALSE;
    refresh_controls(installed ? "Installed. Opening Krutrim Mulyankan…" : NULL);
    if (error) show_error(error);
}

static void open_clicked(GtkButton *button, gpointer user_data) {
    GError *error = NULL;
    if (!open_app(&error)) show_error(error);
    else refresh_controls("Opening Krutrim Mulyankan…");
}

static void remove_clicked(GtkButton *button, gpointer user_data) {
    GtkWidget *dialog = gtk_message_dialog_new(GTK_WINDOW(window), GTK_DIALOG_MODAL,
        GTK_MESSAGE_QUESTION, GTK_BUTTONS_NONE, "Remove Krutrim Mulyankan?");
    gtk_message_dialog_format_secondary_text(GTK_MESSAGE_DIALOG(dialog),
        "The app and its menu entry will be removed. Evaluation results and saved keys will be kept.");
    gtk_dialog_add_buttons(GTK_DIALOG(dialog), "Cancel", GTK_RESPONSE_CANCEL, "Remove", GTK_RESPONSE_ACCEPT, NULL);
    gint response = gtk_dialog_run(GTK_DIALOG(dialog));
    gtk_widget_destroy(dialog);
    if (response != GTK_RESPONSE_ACCEPT) return;
    GError *error = NULL;
    if (!remove_app(&error)) {
        show_error(error);
        refresh_controls(NULL);
    } else refresh_controls("The app was removed. Your evaluation results and saved keys were kept.");
}

static gboolean prevent_close_while_installing(GtkWidget *widget, GdkEvent *event, gpointer user_data) {
    return busy;
}

static int graphical_setup(int argc, char **argv) {
    if (!gtk_init_check(&argc, &argv)) {
        g_printerr("Open this setup from Files in your graphical desktop session.\n");
        return 1;
    }
    window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(window), "Krutrim Mulyankan Setup");
    gtk_window_set_default_size(GTK_WINDOW(window), 540, 350);
    gtk_window_set_position(GTK_WINDOW(window), GTK_WIN_POS_CENTER);
    gtk_container_set_border_width(GTK_CONTAINER(window), 30);
    g_signal_connect(window, "destroy", G_CALLBACK(gtk_main_quit), NULL);
    g_signal_connect(window, "delete-event", G_CALLBACK(prevent_close_while_installing), NULL);
    GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 14);
    gtk_container_add(GTK_CONTAINER(window), box);
    const char *icon = g_file_test(paths.payload_icon, G_FILE_TEST_IS_REGULAR) ? paths.payload_icon : paths.icon;
    GdkPixbuf *image = gdk_pixbuf_new_from_file_at_scale(icon, 72, 72, TRUE, NULL);
    if (image) {
        gtk_box_pack_start(GTK_BOX(box), gtk_image_new_from_pixbuf(image), FALSE, FALSE, 0);
        gtk_window_set_icon(GTK_WINDOW(window), image);
        g_object_unref(image);
    }
    GtkWidget *title = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(title), "<span size='xx-large' weight='bold'>Krutrim Mulyankan</span>");
    gtk_box_pack_start(GTK_BOX(box), title, FALSE, FALSE, 0);
    GtkWidget *version = gtk_label_new("Linux desktop app · " KM_VERSION);
    gtk_style_context_add_class(gtk_widget_get_style_context(version), "dim-label");
    gtk_box_pack_start(GTK_BOX(box), version, FALSE, FALSE, 0);
    status_label = gtk_label_new(NULL);
    gtk_label_set_line_wrap(GTK_LABEL(status_label), TRUE);
    gtk_label_set_max_width_chars(GTK_LABEL(status_label), 55);
    gtk_label_set_justify(GTK_LABEL(status_label), GTK_JUSTIFY_CENTER);
    gtk_box_pack_start(GTK_BOX(box), status_label, TRUE, TRUE, 12);
    GtkWidget *actions = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
    gtk_widget_set_halign(actions, GTK_ALIGN_CENTER);
    gtk_box_pack_end(GTK_BOX(box), actions, FALSE, FALSE, 0);
    install_button = gtk_button_new_with_label("Install & Open");
    open_button = gtk_button_new_with_label("Open");
    remove_button = gtk_button_new_with_label("Remove");
    close_button = gtk_button_new_with_label("Close");
    gtk_style_context_add_class(gtk_widget_get_style_context(install_button), "suggested-action");
    gtk_style_context_add_class(gtk_widget_get_style_context(remove_button), "destructive-action");
    gtk_box_pack_start(GTK_BOX(actions), install_button, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(actions), open_button, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(actions), remove_button, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(actions), close_button, FALSE, FALSE, 0);
    g_signal_connect(install_button, "clicked", G_CALLBACK(install_clicked), NULL);
    g_signal_connect(open_button, "clicked", G_CALLBACK(open_clicked), NULL);
    g_signal_connect(remove_button, "clicked", G_CALLBACK(remove_clicked), NULL);
    g_signal_connect_swapped(close_button, "clicked", G_CALLBACK(gtk_widget_destroy), window);
    gtk_widget_show_all(window);
    refresh_controls(NULL);
    gtk_main();
    return 0;
}

int main(int argc, char **argv) {
    gboolean install = FALSE, uninstall = FALSE, no_launch = FALSE;
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--version") == 0) { puts(KM_VERSION); return 0; }
        if (strcmp(argv[i], "--install") == 0) install = TRUE;
        else if (strcmp(argv[i], "--uninstall") == 0) uninstall = TRUE;
        else if (strcmp(argv[i], "--no-launch") == 0) no_launch = TRUE;
        else if (strcmp(argv[i], "--manage") == 0) manage_only = TRUE;
        else { g_printerr("Unknown setup option: %s\n", argv[i]); return 2; }
    }
    if ((install && uninstall) || (no_launch && !install) || (manage_only && (install || uninstall))) {
        g_printerr("Invalid setup options.\n"); return 2;
    }
    GError *error = NULL;
    if (!init_paths(&error)) { g_printerr("%s\n", error->message); g_error_free(error); return 1; }
    if (install || uninstall) {
        gboolean ok = install ? install_app(&error) : remove_app(&error);
        if (ok && install && !no_launch) ok = open_app(&error);
        if (!ok) { g_printerr("%s\n", error->message); g_error_free(error); return 1; }
        puts(install ? "Krutrim Mulyankan installed." : "Krutrim Mulyankan removed; results and saved keys kept.");
        return 0;
    }
    /* GTK receives no private setup switches. */
    return graphical_setup(1, argv);
}
