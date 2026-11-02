/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Programmable rendering shared by the ES 2.0 backends.

use super::gles11_raw::types::*;

/// Programmable operations on an active context. Pointer arguments must cover
/// the number of elements specified by the corresponding OpenGL call.
#[allow(clippy::missing_safety_doc, clippy::too_many_arguments)]
pub trait Programmable {
    /// Upload complete shader source, with explicit length (no NUL required).
    unsafe fn ShaderSource(&mut self, shader: GLuint, source: &[u8]);
    unsafe fn CreateShader(&mut self, type_: GLenum) -> GLuint;
    unsafe fn CreateProgram(&mut self) -> GLuint;
    unsafe fn CompileShader(&mut self, shader: GLuint);
    unsafe fn AttachShader(&mut self, program: GLuint, shader: GLuint);
    unsafe fn BindAttribLocation(
        &mut self,
        program: GLuint,
        index: GLuint,
        name: *const std::ffi::c_char,
    );
    unsafe fn LinkProgram(&mut self, program: GLuint);
    unsafe fn UseProgram(&mut self, program: GLuint);
    unsafe fn DeleteShader(&mut self, shader: GLuint);
    unsafe fn DeleteProgram(&mut self, program: GLuint);
    unsafe fn GetShaderiv(&mut self, shader: GLuint, pname: GLenum, params: *mut GLint);
    unsafe fn GetProgramiv(&mut self, program: GLuint, pname: GLenum, params: *mut GLint);
    unsafe fn GetShaderInfoLog(
        &mut self,
        shader: GLuint,
        capacity: GLsizei,
        length: *mut GLsizei,
        log: *mut std::ffi::c_char,
    );
    unsafe fn GetProgramInfoLog(
        &mut self,
        program: GLuint,
        capacity: GLsizei,
        length: *mut GLsizei,
        log: *mut std::ffi::c_char,
    );
    unsafe fn GetUniformLocation(
        &mut self,
        program: GLuint,
        name: *const std::ffi::c_char,
    ) -> GLint;
    unsafe fn GetAttribLocation(&mut self, program: GLuint, name: *const std::ffi::c_char)
        -> GLint;
    unsafe fn EnableVertexAttribArray(&mut self, index: GLuint);
    unsafe fn DisableVertexAttribArray(&mut self, index: GLuint);
    unsafe fn VertexAttribPointer(
        &mut self,
        index: GLuint,
        size: GLint,
        type_: GLenum,
        normalized: GLboolean,
        stride: GLsizei,
        pointer: *const GLvoid,
    );
    unsafe fn VertexAttrib4fv(&mut self, index: GLuint, value: *const GLfloat);
    unsafe fn Uniform1i(&mut self, location: GLint, value: GLint);
    unsafe fn Uniform1fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat);
    unsafe fn Uniform2fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat);
    unsafe fn Uniform3fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat);
    unsafe fn Uniform4fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat);
    unsafe fn UniformMatrix2fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    );
    unsafe fn UniformMatrix3fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    );
    unsafe fn UniformMatrix4fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    );
    unsafe fn Uniform1iv(&mut self, location: GLint, count: GLsizei, value: *const GLint);
    unsafe fn Uniform2iv(&mut self, location: GLint, count: GLsizei, value: *const GLint);
    unsafe fn Uniform3iv(&mut self, location: GLint, count: GLsizei, value: *const GLint);
    unsafe fn Uniform4iv(&mut self, location: GLint, count: GLsizei, value: *const GLint);
    unsafe fn ValidateProgram(&mut self, program: GLuint);
    unsafe fn BlendColor(&mut self, red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat);
    unsafe fn BlendEquationSeparate(&mut self, mode_rgb: GLenum, mode_alpha: GLenum);
    unsafe fn BlendFuncSeparate(
        &mut self,
        src_rgb: GLenum,
        dst_rgb: GLenum,
        src_alpha: GLenum,
        dst_alpha: GLenum,
    );
    // OES_vertex_array_object
    unsafe fn GenVertexArrays(&mut self, n: GLsizei, arrays: *mut GLuint);
    unsafe fn BindVertexArray(&mut self, array: GLuint);
    unsafe fn DeleteVertexArrays(&mut self, n: GLsizei, arrays: *const GLuint);
}

/// GLSL ES 1.00 only allows ASCII characters, but iPhone OS's compiler accepts
/// others in comments (e.g. Monster Hunter Portable 2nd G's shaders have
/// Japanese comments), while stricter drivers (e.g. Qualcomm's) reject the
/// shader. Outside comments they're invalid anyway, so replace them with
/// spaces.
pub fn ascii_shader_source(source: &[u8]) -> Vec<u8> {
    source
        .iter()
        .map(|&byte| if byte.is_ascii() { byte } else { b' ' })
        .collect()
}

/// Emulate EXT_shadow_samplers for drivers that don't support it in OpenGL ES
/// 2.0 (e.g. Qualcomm's), by sampling the depth texture normally and doing
/// the depth comparison (GL_LEQUAL) in the shader.
pub fn emulate_shadow_samplers(source: &[u8]) -> Vec<u8> {
    let source = String::from_utf8_lossy(source);
    if !source.contains("sampler2DShadow") {
        return source.into_owned().into_bytes();
    }
    let mut output = String::from(
        "#define Podroid_shadow2D(s, c) step((c).z, texture2D(s, (c).xy).r)\n\
         #define Podroid_shadow2DProj(s, c) step((c).z / (c).w, texture2DProj(s, c).r)\n",
    );
    // Preserve newlines for useful compiler line numbers.
    for line in source.lines() {
        if line
            .trim_start()
            .starts_with("#extension GL_EXT_shadow_samplers")
        {
            output.push('\n');
            continue;
        }
        output.push_str(
            &line
                .replace("sampler2DShadow", "sampler2D")
                .replace("shadow2DProjEXT", "Podroid_shadow2DProj")
                .replace("shadow2DEXT", "Podroid_shadow2D"),
        );
        output.push('\n');
    }
    output.into_bytes()
}

/// GLSL ES 1.00 is close to desktop GLSL 1.20, except for precision
/// declarations and version/extension directives.
pub fn desktop_shader_source(source: &[u8]) -> Vec<u8> {
    let source = String::from_utf8_lossy(source);
    let mut output = String::from("#version 120\n#define lowp\n#define mediump\n#define highp\n");
    // EXT_shadow_samplers: desktop GLSL 1.20 has the same functions, without
    // the suffix and returning a vec4.
    output.push_str("#define shadow2DEXT(s, c) shadow2D(s, c).r\n");
    output.push_str("#define shadow2DProjEXT(s, c) shadow2DProj(s, c).r\n");
    // Preserve newlines for useful compiler line numbers.
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#version")
            || trimmed.starts_with("#extension GL_OES_standard_derivatives")
            || trimmed.starts_with("#extension GL_EXT_shadow_samplers")
        {
            output.push('\n');
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    // A precision statement may share a line with declarations. Remove only
    // that statement rather than discarding the rest of the line.
    while let Some(start) = output.find("precision ") {
        let Some(end) = output[start..].find(';') else {
            break;
        };
        output.replace_range(start..start + end + 1, "");
    }
    output.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::desktop_shader_source;

    #[test]
    fn translates_es_version_and_precision_without_losing_declarations() {
        let source = b"#version 100\nprecision mediump float; uniform vec4 color;\nvoid main() { gl_FragColor = color; }\n";
        let result = String::from_utf8(desktop_shader_source(source)).unwrap();
        assert!(result.starts_with("#version 120\n"));
        assert!(!result.contains("#version 100"));
        assert!(!result.contains("precision mediump float;"));
        assert!(result.contains("uniform vec4 color;"));
        assert!(result.contains("gl_FragColor = color;"));
    }

    #[test]
    fn desktop_derivatives_do_not_require_the_es_extension() {
        let result = String::from_utf8(desktop_shader_source(
            b"#extension GL_OES_standard_derivatives : enable\nvarying highp vec2 uv;\nvoid main() { gl_FragColor = vec4(dFdx(uv), 0.0, 1.0); }",
        )).unwrap();
        assert!(!result.contains("#extension GL_OES_standard_derivatives"));
        assert!(result.contains("#define highp\n"));
        assert!(result.contains("dFdx(uv)"));
    }

    #[test]
    fn shadow_samplers_are_emulated() {
        let result = String::from_utf8(super::emulate_shadow_samplers(
            b" #extension GL_EXT_shadow_samplers : enable\nprecision mediump float;\nuniform sampler2DShadow s;\nvarying highp vec4 c;\nvoid main() { gl_FragColor = vec4(shadow2DProjEXT(s, c)); }\n",
        ))
        .unwrap();
        assert!(!result.contains("GL_EXT_shadow_samplers"));
        assert!(!result.contains("sampler2DShadow"));
        assert!(result.contains("uniform sampler2D s;"));
        assert!(result.contains("vec4(Podroid_shadow2DProj(s, c))"));
        // Shaders without shadow samplers are unchanged.
        assert_eq!(
            super::emulate_shadow_samplers(b"void main() {}"),
            b"void main() {}"
        );
    }

    #[test]
    fn non_ascii_characters_are_replaced() {
        let source = "// 通常モード\nvoid main() {}\n".as_bytes();
        let result = super::ascii_shader_source(source);
        assert!(result.is_ascii());
        assert!(String::from_utf8(result)
            .unwrap()
            .ends_with("\nvoid main() {}\n"));
    }

    #[test]
    fn shadow_samplers_map_to_desktop_functions() {
        let result = String::from_utf8(desktop_shader_source(
            b"#extension GL_EXT_shadow_samplers : require\nuniform sampler2DShadow s;\nvarying highp vec4 c;\nvoid main() { gl_FragColor = vec4(shadow2DProjEXT(s, c)); }",
        )).unwrap();
        assert!(!result.contains("#extension GL_EXT_shadow_samplers"));
        assert!(result.contains("#define shadow2DProjEXT(s, c) shadow2DProj(s, c).r\n"));
    }
}
