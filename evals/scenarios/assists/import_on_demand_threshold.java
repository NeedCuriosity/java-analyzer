//- /.editorconfig
[*.java]
ij_java_class_count_to_use_import_on_demand = 3
//- /app/Collapse.java
package app;

import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentMap;
import java.util.concurrent.CopyOnWriteArrayList;

class /*@collapse*/Collapse {
    ConcurrentMap<String, String> a = new ConcurrentHashMap<>();
    CopyOnWriteArrayList<String> b;
}
//- /app/Expand.java
package app;

import java.time.*;

class /*@expand*/Expand {
    LocalDate a;
}
//- /app/Optional.java
package app;

public class Optional {}
//- /app/Conflicts.java
package app;

import java.awt.*;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.Set;

class /*@conflicts*/Conflicts {
    List<String> a = new ArrayList<>();
    Set<Optional<String>> b;
    Color c;
    Frame f;
    Panel p;
}
//? assist organize_imports collapse ; has "import java.util.concurrent.*;" ; lacks "import java.util.concurrent.ConcurrentMap;"
//? assist organize_imports expand ; has "import java.time.LocalDate;" ; lacks "import java.time.*;"
//? assist organize_imports conflicts ; has "import java.awt.*;" ; has "import java.util.*;" ; has "import java.util.List;" ; has "import java.util.Optional;"
