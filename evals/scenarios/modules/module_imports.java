//- /app/ModImport.java
package app;

import module java.base;
import module java.sql;

public class ModImport {
    List<String> names = new ArrayList<>();
    Map<String, Integer> counts = new HashMap<>();
    Connection /*@conn*/connection;
    /*@date*/Date when;
    java.util.Date utilDate;

    void use() {
        Path path = Path.of("x");
        names.add(path.toString());
    }
}
//- /app/Desktop.java
package app;

import module java.base;
import module java.desktop;
import java.util.List;

class Desktop {
    // 单类型 import 消除了 java.util.List / java.awt.List 的歧义
    List<String> items;
    /*@frame*/JFrame frame;
}
//- /app/Missing.java
package app;

import module /*@missing*/java.missing;

class Missing {}
//? hover conn ~ "Connection connection"
//? diag unresolved-type date
//? diag unresolved-import missing
//? diag exact
