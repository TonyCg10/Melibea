# A green window with a magenta box of exactly 300x200 logical pixels centred in it.
# Used by nest.sh: measuring anywhere inside the box must give 300x200 at any scale.
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, Gdk

css = b"""
window { background: #00ff00; }
.target { background: #ff00ff; }
"""

def on_activate(app):
    provider = Gtk.CssProvider()
    provider.load_from_data(css)
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
    box = Gtk.Box()
    box.add_css_class("target")
    box.set_size_request(300, 200)
    box.set_halign(Gtk.Align.CENTER)
    box.set_valign(Gtk.Align.CENTER)
    window = Gtk.ApplicationWindow(application=app, title="RULER-PROBE")
    window.set_child(box)
    window.present()

app = Gtk.Application(application_id="org.melibea.RulerProbe")
app.connect("activate", on_activate)
app.run(None)
