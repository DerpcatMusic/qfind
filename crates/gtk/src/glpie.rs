//! GPU sunburst renderer for the storage chart.
//!
//! The pie is tessellated once per dataset into unit-space triangles and
//! drawn with a single OpenGL program: one draw call for the fills, one for
//! the separators. Resizing updates uniforms; hover uses the cursor and tooltip
//! without redrawing the chart. When GL
//! is unavailable the stack flips to an explanatory label instead.

use std::cell::RefCell;
use std::rc::Rc;

use glow::HasContext as _;
use gtk::glib;
use gtk::prelude::*;

const VERT_BODY: &str = "
in vec2 a_pos;
in vec3 a_color;
in float a_id;
out vec3 v_color;
out float v_id;
uniform vec2 u_center;
uniform float u_scale;
uniform vec2 u_viewport;
void main() {
    v_color = a_color;
    v_id = a_id;
    vec2 px = a_pos * u_scale + u_center;
    gl_Position = vec4(
        px.x / u_viewport.x * 2.0 - 1.0,
        1.0 - px.y / u_viewport.y * 2.0,
        0.0,
        1.0
    );
}";

const FRAG_BODY: &str = "
in vec3 v_color;
in float v_id;
out vec4 f_color;
void main() {
    f_color = vec4(v_color, 1.0);
}";

/// Both shader bodies compile as desktop GLSL 1.30 and as ES 3.00 (the ES
/// fragment shader needs an explicit float precision). The context decides:
/// real desktops hand out desktop GL, headless/embedded stacks hand out ES.
fn shader_sources(es: bool) -> (String, String) {
    if es {
        (
            format!("#version 300 es\n{VERT_BODY}"),
            format!("#version 300 es\nprecision mediump float;\n{FRAG_BODY}"),
        )
    } else {
        (
            format!("#version 130\n{VERT_BODY}"),
            format!("#version 130\n{FRAG_BODY}"),
        )
    }
}

/// One sunburst slice in unit space (center 0,0, outer radius <= 1).
pub(crate) struct SliceGeom {
    pub start: f64,
    pub end: f64,
    pub inner: f64,
    pub outer: f64,
    pub color: (f64, f64, f64),
    pub id: u32,
}

/// Tessellate slices into interleaved `[x, y, r, g, b, id]` triangle and line
/// vertex lists.
pub(crate) fn tessellate(slices: &[SliceGeom]) -> (Vec<f32>, Vec<f32>) {
    let mut tris = Vec::new();
    let mut lines = Vec::new();
    for slice in slices {
        let span = (slice.end - slice.start).max(0.0);
        if span <= 0.0 {
            continue;
        }
        let segments = ((span / std::f64::consts::TAU * 160.0).ceil() as usize).clamp(4, 96);
        let (r, g, b) = slice.color;
        let id = slice.id as f32;
        let point = |angle: f64, radius: f64| {
            [(radius * angle.cos()) as f32, (radius * angle.sin()) as f32]
        };
        let mut prev_outer = point(slice.start, slice.outer);
        let mut prev_inner = point(slice.start, slice.inner);
        lines.extend_from_slice(&[prev_inner[0], prev_inner[1], 0.03, 0.03, 0.05, -1.0]);
        lines.extend_from_slice(&[prev_outer[0], prev_outer[1], 0.03, 0.03, 0.05, -1.0]);
        for step in 1..=segments {
            let angle = slice.start + span * step as f64 / segments as f64;
            let outer = point(angle, slice.outer);
            let inner = point(angle, slice.inner);
            for v in [prev_inner, prev_outer, outer, prev_inner, outer, inner] {
                tris.extend_from_slice(&[v[0], v[1], r as f32, g as f32, b as f32, id]);
            }
            for (a, c) in [(prev_outer, outer), (prev_inner, inner)] {
                lines.extend_from_slice(&[a[0], a[1], 0.03, 0.03, 0.05, -1.0]);
                lines.extend_from_slice(&[c[0], c[1], 0.03, 0.03, 0.05, -1.0]);
            }
            prev_outer = outer;
            prev_inner = inner;
        }
        lines.extend_from_slice(&[prev_inner[0], prev_inner[1], 0.03, 0.03, 0.05, -1.0]);
        lines.extend_from_slice(&[prev_outer[0], prev_outer[1], 0.03, 0.03, 0.05, -1.0]);
    }
    (tris, lines)
}

fn bytes_of(values: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, values.len() * 4) }
}

/// Tessellated unit-space vertex lists: (fill triangles, separator lines).
type MeshData = (Vec<f32>, Vec<f32>);
/// Locations of the (center, scale, viewport) uniforms.
type PieUniforms = (
    glow::UniformLocation,
    glow::UniformLocation,
    glow::UniformLocation,
);

/// Load OpenGL symbols from the platform GL library. glow resolves every
/// function pointer eagerly inside `from_loader_function`, so the library
/// only needs to stay open for the duration of this call.
fn load_gl() -> Result<glow::Context, String> {
    use std::ffi::{CString, c_char, c_void};

    // SAFETY: opening a well-known system library by name.
    #[cfg(target_os = "linux")]
    {
        type GetProc = unsafe extern "C" fn(*const c_char) -> *mut c_void;
        let lib = unsafe { libloading::Library::new("libGL.so.1") }
            .map_err(|err| format!("system OpenGL library: {err}"))?;
        let get_proc: libloading::Symbol<GetProc> = unsafe {
            lib.get(b"glXGetProcAddressARB")
                .map_err(|err| format!("glXGetProcAddressARB: {err}"))?
        };
        let get_proc = *get_proc;
        Ok(unsafe {
            glow::Context::from_loader_function(|name| {
                let cname = CString::new(name).unwrap_or_default();
                get_proc(cname.as_ptr()) as *const _
            })
        })
    }
    // SAFETY: opening a well-known system library by path.
    #[cfg(target_os = "macos")]
    {
        let lib = unsafe {
            libloading::Library::new("/System/Library/Frameworks/OpenGL.framework/OpenGL")
        }
        .map_err(|err| format!("system OpenGL library: {err}"))?;
        Ok(glow::Context::from_loader_function(|name| {
            lib.get::<*const c_void>(name.as_bytes())
                .map(|symbol| *symbol as *const _)
                .unwrap_or(std::ptr::null())
        }))
    }
    // SAFETY: opening a well-known system library by name.
    #[cfg(target_os = "windows")]
    {
        type GetProc = unsafe extern "C" fn(*const c_char) -> *mut c_void;
        let lib = unsafe { libloading::Library::new("opengl32.dll") }
            .map_err(|err| format!("system OpenGL library: {err}"))?;
        let wgl: libloading::Symbol<GetProc> = unsafe {
            lib.get(b"wglGetProcAddress")
                .map_err(|err| format!("wglGetProcAddress: {err}"))?
        };
        let wgl = *wgl;
        Ok(glow::Context::from_loader_function(|name| {
            let cname = CString::new(name).unwrap_or_default();
            let ptr = wgl(cname.as_ptr());
            if ptr.is_null() {
                lib.get::<*mut c_void>(name.as_bytes())
                    .map(|symbol| *symbol as *const _)
                    .unwrap_or(std::ptr::null())
            } else {
                ptr as *const _
            }
        }))
    }
}

struct GlState {
    ctx: Option<Rc<glow::Context>>,
    program: Option<glow::Program>,
    vao_fill: Option<glow::NativeVertexArray>,
    vbo_fill: Option<glow::NativeBuffer>,
    fill_count: i32,
    vao_line: Option<glow::NativeVertexArray>,
    vbo_line: Option<glow::NativeBuffer>,
    line_count: i32,
    loc_center: Option<glow::UniformLocation>,
    loc_scale: Option<glow::UniformLocation>,
    loc_viewport: Option<glow::UniformLocation>,
    center: (f32, f32),
    scale: f32,
    viewport: (i32, i32),
}

impl GlState {
    fn empty() -> Self {
        Self {
            ctx: None,
            program: None,
            vao_fill: None,
            vbo_fill: None,
            fill_count: 0,
            vao_line: None,
            vbo_line: None,
            line_count: 0,
            loc_center: None,
            loc_scale: None,
            loc_viewport: None,
            center: (0.0, 0.0),
            scale: 1.0,
            viewport: (1, 1),
        }
    }
}

#[derive(Clone)]
pub(crate) struct GlPie {
    pub root: gtk::Stack,
    pub view: gtk::Overlay,
    pub gl: gtk::GLArea,
    pub labels: gtk::DrawingArea,
    state: Rc<RefCell<GlState>>,
    pending: Rc<RefCell<Option<MeshData>>>,
}

impl GlPie {
    pub(crate) fn new() -> Self {
        let gl = gtk::GLArea::new();
        gl.set_hexpand(true);
        gl.set_vexpand(true);
        gl.set_focusable(true);
        gl.set_auto_render(false);
        gl.set_has_depth_buffer(false);
        gl.set_has_stencil_buffer(false);

        let labels = gtk::DrawingArea::new();
        labels.set_hexpand(true);
        labels.set_vexpand(true);
        labels.set_halign(gtk::Align::Fill);
        labels.set_valign(gtk::Align::Fill);

        let view = gtk::Overlay::new();
        view.set_child(Some(&gl));
        view.add_overlay(&labels);

        let error = gtk::Label::new(Some(
            "Storage chart needs OpenGL 3.0 or newer; the file views are unaffected.",
        ));
        error.set_wrap(true);
        error.set_halign(gtk::Align::Center);
        error.set_valign(gtk::Align::Center);

        let root = gtk::Stack::new();
        root.set_vexpand(true);
        root.add_named(&view, Some("gl"));
        root.add_named(&error, Some("gl-error"));

        let pie = Self {
            root,
            view,
            gl,
            labels,
            state: Rc::new(RefCell::new(GlState::empty())),
            pending: Rc::new(RefCell::new(None)),
        };
        pie.connect_realize();
        pie.connect_render();
        pie.connect_resize();
        pie
    }

    fn fail(&self, message: String) {
        eprintln!("qfind: GL pie disabled: {message}");
        self.root.set_visible_child_name("gl-error");
    }

    fn connect_realize(&self) {
        let pie = self.clone();
        self.gl.connect_realize(move |area| {
            if let Some(error) = area.error() {
                pie.fail(format!("no GL context ({error})"));
                return;
            }
            area.make_current();
            let ctx = match load_gl() {
                Ok(ctx) => ctx,
                Err(message) => {
                    pie.fail(message);
                    return;
                }
            };
            match unsafe { init_program(&ctx) } {
                Ok((program, locations)) => {
                    let version = unsafe { ctx.get_parameter_string(glow::VERSION) };
                    eprintln!("qfind: GL pie on {version}");
                    let mut state = pie.state.borrow_mut();
                    state.ctx = Some(Rc::new(ctx));
                    state.program = Some(program);
                    state.loc_center = Some(locations.0);
                    state.loc_scale = Some(locations.1);
                    state.loc_viewport = Some(locations.2);
                    drop(state);
                    if let Some((tris, lines)) = pie.pending.borrow_mut().take() {
                        pie.upload(&tris, &lines);
                    }
                }
                Err(message) => pie.fail(message),
            }
        });
    }

    fn connect_render(&self) {
        let state = Rc::clone(&self.state);
        self.gl.connect_render(move |_, _| unsafe {
            let state = state.borrow();
            let (Some(ctx), Some(program)) = (state.ctx.clone(), state.program) else {
                return glib::Propagation::Proceed;
            };
            ctx.viewport(0, 0, state.viewport.0, state.viewport.1);
            ctx.clear_color(0.0, 0.0, 0.0, 0.0);
            ctx.clear(glow::COLOR_BUFFER_BIT);
            ctx.use_program(Some(program));
            ctx.uniform_2_f32(state.loc_center.as_ref(), state.center.0, state.center.1);
            ctx.uniform_1_f32(state.loc_scale.as_ref(), state.scale);
            ctx.uniform_2_f32(
                state.loc_viewport.as_ref(),
                state.viewport.0 as f32,
                state.viewport.1 as f32,
            );
            if state.fill_count > 0 {
                ctx.bind_vertex_array(state.vao_fill);
                ctx.draw_arrays(glow::TRIANGLES, 0, state.fill_count);
            }
            if state.line_count > 0 {
                ctx.bind_vertex_array(state.vao_line);
                ctx.draw_arrays(glow::LINES, 0, state.line_count);
            }
            ctx.bind_vertex_array(None);
            ctx.use_program(None);
            glib::Propagation::Proceed
        });
    }

    fn connect_resize(&self) {
        let state = Rc::clone(&self.state);
        self.gl.connect_resize(move |_, width, height| {
            state.borrow_mut().viewport = (width.max(1), height.max(1));
        });
    }

    fn upload(&self, tris: &[f32], lines: &[f32]) {
        self.gl.make_current();
        let mut state = self.state.borrow_mut();
        let Some(ctx) = state.ctx.clone() else {
            return;
        };
        // SAFETY: buffers are created from this context on the main thread.
        unsafe {
            let (vao_fill, vbo_fill) = match (state.vao_fill, state.vbo_fill) {
                (Some(vao), Some(vbo)) => {
                    ctx.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    ctx.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(tris), glow::STATIC_DRAW);
                    (vao, vbo)
                }
                _ => bind_attribs(&ctx, tris),
            };
            state.vao_fill = Some(vao_fill);
            state.vbo_fill = Some(vbo_fill);
            state.fill_count = (tris.len() / 6) as i32;
            let (vao_line, vbo_line) = match (state.vao_line, state.vbo_line) {
                (Some(vao), Some(vbo)) => {
                    ctx.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    ctx.buffer_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        bytes_of(lines),
                        glow::STATIC_DRAW,
                    );
                    (vao, vbo)
                }
                _ => bind_attribs(&ctx, lines),
            };
            state.vao_line = Some(vao_line);
            state.vbo_line = Some(vbo_line);
            state.line_count = (lines.len() / 6) as i32;
            ctx.bind_vertex_array(None);
            ctx.bind_buffer(glow::ARRAY_BUFFER, None);
        }
    }

    /// Replace the unit-space geometry. Uploads immediately when realized,
    /// otherwise stashes it for the realize handler.
    pub(crate) fn set_geometry(&self, tris: Vec<f32>, lines: Vec<f32>) {
        if self.state.borrow().ctx.is_some() {
            self.upload(&tris, &lines);
            self.gl.queue_render();
        } else {
            *self.pending.borrow_mut() = Some((tris, lines));
        }
    }

    /// Pixel-space view transform: allocation center, pixels per unit radius,
    /// and framebuffer size for the resize handler.
    pub(crate) fn set_view(&self, cx: f32, cy: f32, scale: f32) {
        let mut state = self.state.borrow_mut();
        state.center = (cx, cy);
        state.scale = scale.max(1.0);
    }
}

/// SAFETY: the context must be current on the calling thread and `data`
/// must hold `[x, y, r, g, b, id]` sextuples.
unsafe fn bind_attribs(
    ctx: &glow::Context,
    data: &[f32],
) -> (glow::NativeVertexArray, glow::NativeBuffer) {
    unsafe {
        let vao = ctx.create_vertex_array().expect("pie vertex array");
        let vbo = ctx.create_buffer().expect("pie vertex buffer");
        ctx.bind_vertex_array(Some(vao));
        ctx.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        ctx.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(data), glow::STATIC_DRAW);
        let stride = 6 * 4;
        ctx.enable_vertex_attrib_array(0);
        ctx.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        ctx.enable_vertex_attrib_array(1);
        ctx.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 2 * 4);
        ctx.enable_vertex_attrib_array(2);
        ctx.vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, stride, 5 * 4);
        (vao, vbo)
    }
}

/// SAFETY: the context must be current on the calling thread.
unsafe fn compile(
    ctx: &glow::Context,
    kind: u32,
    source: &str,
) -> Result<glow::NativeShader, String> {
    unsafe {
        let shader = ctx
            .create_shader(kind)
            .map_err(|message| format!("shader object: {message}"))?;
        ctx.shader_source(shader, source);
        ctx.compile_shader(shader);
        if ctx.get_shader_compile_status(shader) {
            Ok(shader)
        } else {
            let log = ctx.get_shader_info_log(shader);
            ctx.delete_shader(shader);
            Err(format!("compile: {log}"))
        }
    }
}

/// SAFETY: the context must be current on the calling thread.
unsafe fn init_program(ctx: &glow::Context) -> Result<(glow::Program, PieUniforms), String> {
    unsafe {
        let version = ctx.get_parameter_string(glow::VERSION);
        let es = version.contains("OpenGL ES");
        // Try the matching profile first, then the other one: some stacks
        // misreport, and a second attempt costs nothing at startup.
        let mut logs = Vec::new();
        for attempt_es in [es, !es] {
            let (vert_source, frag_source) = shader_sources(attempt_es);
            match try_program(ctx, &vert_source, &frag_source) {
                Ok(program) => return Ok(program),
                Err(log) => logs.push(log),
            }
        }
        eprintln!("qfind: GL version string: {version}");
        Err(logs.join(" | "))
    }
}

/// SAFETY: the context must be current on the calling thread.
unsafe fn try_program(
    ctx: &glow::Context,
    vert_source: &str,
    frag_source: &str,
) -> Result<(glow::Program, PieUniforms), String> {
    unsafe {
        let vert = compile(ctx, glow::VERTEX_SHADER, vert_source)?;
        let frag = compile(ctx, glow::FRAGMENT_SHADER, frag_source)?;
        let program = ctx
            .create_program()
            .map_err(|message| format!("program object: {message}"))?;
        ctx.attach_shader(program, vert);
        ctx.attach_shader(program, frag);
        ctx.bind_attrib_location(program, 0, "a_pos");
        ctx.bind_attrib_location(program, 1, "a_color");
        ctx.bind_attrib_location(program, 2, "a_id");
        ctx.link_program(program);
        ctx.delete_shader(vert);
        ctx.delete_shader(frag);
        if !ctx.get_program_link_status(program) {
            let log = ctx.get_program_info_log(program);
            ctx.delete_program(program);
            return Err(format!("link: {log}"));
        }
        // Uniform locations are queried eagerly; the closure only borrows.
        let center = ctx
            .get_uniform_location(program, "u_center")
            .ok_or_else(|| "missing uniform u_center".to_owned())?;
        let scale = ctx
            .get_uniform_location(program, "u_scale")
            .ok_or_else(|| "missing uniform u_scale".to_owned())?;
        let viewport = ctx
            .get_uniform_location(program, "u_viewport")
            .ok_or_else(|| "missing uniform u_viewport".to_owned())?;
        Ok((program, (center, scale, viewport)))
    }
}
