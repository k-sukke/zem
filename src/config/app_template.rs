//! Minimal hello-world application written by `zem init` when the project has none.

pub fn files(project_name: &str) -> [(&'static str, String); 3] {
    [
        (
            "CMakeLists.txt",
            format!(
                "cmake_minimum_required(VERSION 3.20.0)\n\
                 find_package(Zephyr REQUIRED HINTS $ENV{{ZEPHYR_BASE}})\n\
                 project({project_name})\n\
                 \n\
                 target_sources(app PRIVATE src/main.c)\n"
            ),
        ),
        (
            "prj.conf",
            "# Kconfig options for this application\n".to_string(),
        ),
        (
            "src/main.c",
            "#include <stdio.h>\n\
             #include <zephyr/kernel.h>\n\
             \n\
             int main(void)\n\
             {\n\
             \twhile (1) {\n\
             \t\tprintf(\"Hello World! %s\\n\", CONFIG_BOARD_TARGET);\n\
             \t\tk_msleep(1000);\n\
             \t}\n\
             \treturn 0;\n\
             }\n"
            .to_string(),
        ),
    ]
}
