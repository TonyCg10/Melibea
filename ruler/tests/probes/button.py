# A window with one GTK button labelled "Medir". Prints the button's allocated size once it is
# shown, so a person can measure the button with the interactive ruler and compare.
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, GLib

def on_activate(app):
    button = Gtk.Button(label="Medir")
    button.set_halign(Gtk.Align.CENTER)
    button.set_valign(Gtk.Align.CENTER)
    window = Gtk.ApplicationWindow(application=app, title="RULER-BUTTON")
    window.set_child(button)
    window.present()
    GLib.timeout_add(1000, lambda: print(f"button {button.get_width()}x{button.get_height()}", flush=True) or False)

app = Gtk.Application(application_id="org.melibea.RulerButton")
app.connect("activate", on_activate)
app.run(None)
