//- /.editorconfig
root = true

[*]
indent_style = space

[*.{java,kt}]
ij_java_imports_layout = *,|,javax.**,java.**,|,$*
//- /javax/inject/Named.java
package javax.inject;
public @interface Named { String value() default ""; }
//- /org/lib/Thing.java
package org.lib;
public class Thing {}
//- /app/Main.java
package app;

import java.util.List;
import static java.util.Collections.emptyList;
import org.lib.Thing;
import javax.inject.Named;

@Named
class /*@org*/Main {
    List<Thing> things = emptyList();
}
//- /app/Other.java
package app;

import org.lib.Thing;

import javax.inject.Named;
import java.util.List;

@Named
class Other {
    List<Thing> things;
    /*@missing*/ArrayList<String> names;
}
//? assist organize_imports org ; has "import org.lib.Thing;\n\nimport javax.inject.Named;\nimport java.util.List;\n\nimport static java.util.Collections.emptyList;"
//? assist add_import missing ; has "import javax.inject.Named;\nimport java.util.ArrayList;\nimport java.util.List;"
