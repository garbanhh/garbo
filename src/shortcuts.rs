//! Lista de atajos para consultarlos en Ajustes → Atajos: los configurables, con las teclas que
//! tiene el usuario, y los fijos (línea de órdenes, cada vista y ratón) de este sistema.

use crate::keybindings::{key_label, Action};
use std::collections::HashMap;

/// Grupos de los atajos configurables, en el orden en que se enseñan.
const GROUPS: [&str; 5] = [
    "Pestañas",
    "Paneles",
    "Vistas",
    "Texto y bloques",
    "Fuente y tema",
];

/// Grupo de cada acción. Sin comodín: una acción nueva no compila hasta que se le elige grupo.
fn group(a: Action) -> usize {
    use Action::*;
    match a {
        NewTab | CloseTab | NextTab | PrevTab | MoveTabLeft | MoveTabRight | GotoTab(_)
        | RenameTab => 0,
        SplitRight | SplitDown | FocusLeft | FocusRight | FocusUp | FocusDown => 1,
        CommandPalette | Settings | Appearance | ToggleAgents | TogglePorts | ToggleChanges
        | AgentStats | Search => 2,
        Copy | Paste | PrevBlock | NextBlock | CopyBlockOutput | CopyBlockCommand | CopyBlock
        | ScrollPageUp | ScrollPageDown | ScrollTop | ScrollBottom | ClearScrollback => 3,
        FontBigger | FontSmaller | FontReset | NextTheme | ReloadConfig => 4,
    }
}

/// Atajos fijos (no se cambian en la configuración): (título, [(acción, teclas, solo en)]).
/// `Some(true)`: solo en Windows; `Some(false)`: en todos menos Windows.
type Fixed = (&'static str, &'static [&'static str], Option<bool>);
const FIXED: [(&str, &[Fixed]); 3] = [
    (
        "Línea de órdenes",
        &[
            ("Menú de completado", &["Tab", "Ctrl+Espacio"], None),
            ("Aceptar la sugerencia en gris", &["→", "Ctrl+F"], None),
            ("Aceptar solo su siguiente palabra", &["Ctrl+→"], None),
            ("Nueva línea sin ejecutar", &["Shift+Intro"], None),
            ("Borrar la palabra anterior", &["Ctrl+Retroceso"], None),
            ("Borrar la palabra siguiente", &["Ctrl+Supr"], None),
            ("Deshacer", &["Ctrl+Z"], None),
            ("Cambiar de línea en un comando largo", &["↑", "↓"], None),
        ],
    ),
    (
        "Dentro de cada vista",
        &[
            ("Menú de completado: elegir", &["↑", "↓"], None),
            ("Menú de completado: aceptar", &["Intro", "→"], None),
            (
                "Búsqueda: anterior / siguiente",
                &["Intro", "Shift+Intro"],
                None,
            ),
            ("Estadísticas: periodo", &["←", "→", "1 … 4"], None),
            ("Estadísticas: agente", &["Tab"], None),
            ("Ajustes y aspecto: categoría", &["Tab", "Shift+Tab"], None),
            ("Nombre de la pestaña: guardar", &["Intro"], None),
            ("Cerrar la vista, el menú o la búsqueda", &["Esc"], None),
        ],
    ),
    (
        "Ratón",
        &[
            ("Seleccionar una palabra", &["Doble clic"], None),
            ("Seleccionar una línea", &["Triple clic"], None),
            ("Seleccionar un rectángulo", &["Ctrl+Alt+arrastrar"], None),
            ("Forzar la selección", &["Shift+arrastrar"], None),
            ("Abrir un enlace o una ruta", &["Ctrl+clic"], None),
            ("Pegar lo seleccionado", &["Clic central"], Some(false)),
            (
                "Copiar lo seleccionado o pegar",
                &["Clic derecho"],
                Some(true),
            ),
            ("Seleccionar un bloque", &["Clic en el margen"], None),
            ("Renombrar una pestaña", &["Doble clic en ella"], None),
            ("Cerrar una pestaña", &["Clic central en ella"], None),
            ("Abrir una pestaña", &["Clic central en la barra"], None),
            ("Maximizar la ventana", &["Doble clic en la barra"], None),
        ],
    ),
];

/// Teclas de una acción: las de la configuración o las de fábrica; `none` = ninguna.
fn keys_of(spec: &str) -> Vec<String> {
    if spec.trim().eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    spec.split(',')
        .filter(|c| !c.trim().is_empty())
        .map(key_label)
        .collect()
}

/// "Ir a una pestaña": un solo rango (`Alt+1 … Alt+9`) si las nueve siguen el mismo patrón;
/// si no, una fila por pestaña.
fn goto_rows(keys: Vec<Vec<String>>) -> Vec<Row> {
    let prefix = keys[0]
        .first()
        .and_then(|k| k.strip_suffix('1'))
        .unwrap_or_default();
    let pattern = keys
        .iter()
        .enumerate()
        .all(|(i, k)| k.len() == 1 && k[0] == format!("{prefix}{}", i + 1));
    if pattern {
        return vec![Row {
            action: "Ir a una pestaña (9: la última)".into(),
            keys: vec![format!("{prefix}1 … {prefix}9")],
        }];
    }
    keys.into_iter()
        .enumerate()
        .map(|(i, keys)| Row {
            action: if i == 8 {
                "Ir a la pestaña 9 (la última)".into()
            } else {
                format!("Ir a la pestaña {}", i + 1)
            },
            keys,
        })
        .collect()
}

/// Una acción y sus teclas (`keys` vacío: sin atajo).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub action: String,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

/// Todas las secciones, con lo que el usuario cambió en `[keybindings]`.
pub fn sections(user: &HashMap<String, String>) -> Vec<Section> {
    let mut secs: Vec<Section> = GROUPS
        .iter()
        .map(|&title| Section {
            title,
            rows: Vec::new(),
        })
        .collect();
    let mut goto = Vec::new();
    for (action, name, desc, default) in Action::all() {
        let keys = keys_of(user.get(name).map_or(default, String::as_str));
        if matches!(action, Action::GotoTab(_)) {
            goto.push(keys);
            continue;
        }
        secs[group(action)].rows.push(Row {
            action: desc.into(),
            keys,
        });
        // Windows: Ctrl+C copia si hay texto seleccionado (si no, llega al programa).
        if cfg!(windows) && action == Action::Copy {
            secs[group(action)].rows.push(Row {
                action: "Copiar si hay texto seleccionado".into(),
                keys: vec!["Ctrl+C".into()],
            });
        }
    }
    let at = secs[0].rows.len().min(4);
    secs[0].rows.splice(at..at, goto_rows(goto));
    for (title, fixed) in FIXED {
        secs.push(Section {
            title,
            rows: fixed
                .iter()
                .filter(|(.., only)| only.is_none_or(|w| w == cfg!(windows)))
                .map(|(action, keys, _)| Row {
                    action: (*action).into(),
                    keys: keys.iter().map(|k| k.to_string()).collect(),
                })
                .collect(),
        });
    }
    secs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::{key_label, Action};

    fn rows(secs: &[Section]) -> impl Iterator<Item = &Row> {
        secs.iter().flat_map(|s| &s.rows)
    }

    fn find<'a>(secs: &'a [Section], action: &str) -> &'a Row {
        rows(secs)
            .find(|r| r.action == action)
            .unwrap_or_else(|| panic!("falta «{action}»"))
    }

    fn user(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn nombres_de_las_teclas() {
        assert_eq!(key_label("ctrl+shift+pageup"), "Ctrl+Shift+RePág");
        assert_eq!(key_label("ctrl+plus"), "Ctrl++");
        assert_eq!(key_label("ctrl++"), "Ctrl++");
        assert_eq!(key_label("ctrl+equal"), "Ctrl+=");
        assert_eq!(key_label("ctrl+comma"), "Ctrl+,");
        assert_eq!(key_label("shift+insert"), "Shift+Insert");
        assert_eq!(key_label("alt+left"), "Alt+←");
        assert_eq!(key_label("ctrl+shift+alt+d"), "Ctrl+Shift+Alt+D");
        assert_eq!(key_label(" Ctrl + Shift + T "), "Ctrl+Shift+T");
        // Los modificadores, siempre en el mismo orden.
        assert_eq!(key_label("shift+ctrl+tab"), "Ctrl+Shift+Tab");
    }

    #[test]
    fn salen_todas_las_acciones() {
        let secs = sections(&HashMap::new());
        for (a, _, desc, _) in Action::all() {
            if matches!(a, Action::GotoTab(_)) {
                continue;
            }
            let n = rows(&secs).filter(|r| r.action == desc).count();
            assert_eq!(n, 1, "«{desc}» sale {n} veces");
        }
    }

    #[test]
    fn la_configuracion_manda() {
        let defecto = sections(&HashMap::new());
        assert_eq!(
            find(&defecto, "Pestaña siguiente").keys,
            ["Ctrl+Tab", "Ctrl+AvPág"]
        );
        assert!(find(&defecto, "Copiar comando del bloque").keys.is_empty());
        let secs = sections(&user(&[
            ("new_tab", "ctrl+n, ctrl+shift+t"),
            ("settings", "none"),
        ]));
        assert_eq!(
            find(&secs, "Nueva pestaña").keys,
            ["Ctrl+N", "Ctrl+Shift+T"]
        );
        assert!(find(&secs, "Ajustes").keys.is_empty());
    }

    #[test]
    fn ir_a_pestana() {
        let secs = sections(&HashMap::new());
        let ir: Vec<_> = rows(&secs)
            .filter(|r| {
                r.action.starts_with("Ir a una pestaña") || r.action.starts_with("Ir a la pestaña")
            })
            .collect();
        assert_eq!(ir.len(), 1);
        assert_eq!(ir[0].keys, ["Alt+1 … Alt+9"]);
        // Una cambiada: ya no es un rango, una fila por pestaña.
        let secs = sections(&user(&[("goto_tab_3", "ctrl+3")]));
        let ir: Vec<_> = rows(&secs)
            .filter(|r| r.action.starts_with("Ir a la pestaña"))
            .collect();
        assert_eq!(ir.len(), 9);
        assert_eq!(ir[2].keys, ["Ctrl+3"]);
        assert_eq!(ir[0].keys, ["Alt+1"]);
    }

    #[test]
    fn segun_el_sistema() {
        let secs = sections(&HashMap::new());
        let titles: Vec<_> = secs.iter().map(|s| s.title).collect();
        assert_eq!(
            titles,
            [
                "Pestañas",
                "Paneles",
                "Vistas",
                "Texto y bloques",
                "Fuente y tema",
                "Línea de órdenes",
                "Dentro de cada vista",
                "Ratón"
            ]
        );
        let clic_central = rows(&secs).any(|r| r.keys.iter().any(|k| k == "Clic central"));
        let clic_derecho = rows(&secs).any(|r| r.keys.iter().any(|k| k == "Clic derecho"));
        // En Windows no hay selección primaria: el clic central no pega; el derecho copia o pega.
        assert_eq!(clic_central, !cfg!(windows));
        assert_eq!(clic_derecho, cfg!(windows));
    }
}
