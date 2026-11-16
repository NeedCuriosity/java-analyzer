//- /.editorconfig
root = true
[*.java]
indent_size = 4
//- /legacy/.editorconfig
[*.java]
indent_size = 2
ij_java_imports_layout = java.**,|,*
//- /app/Modern.java
package app;

class Modern {
  int f() {
        return 1;
  }
}
//- /app/Modern.java formatted
package app;

class Modern {
    int f() {
        return 1;
    }
}
//- /legacy/Old.java
package legacy;

class Old {
    int f() {
            return 1;
    }
}
//- /legacy/Old.java formatted
package legacy;

class Old {
  int f() {
    return 1;
  }
}
//- /legacy/Imports.java
package legacy;

import org.example.Thing;
import java.util.List;

class /*@imports*/Imports {
  List<Thing> things;
}
//? assist organize_imports imports ; has "import java.util.List;\n\nimport org.example.Thing;"
