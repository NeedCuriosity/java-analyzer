package app;

import java.util.Map;
import java.util.ArrayList;
import java.io.File;

class Main {
    /*@missing*/List<String> names = new ArrayList<>();
    /*@org*/Map<String, Integer> ages;
    /*@known*/String text;
}
//? assist add_import missing ; has "import java.util.List;"
//? assist organize_imports org ; lacks "java.io.File" ; has "import java.util.ArrayList;\nimport java.util.Map;"
//? noassist add_import known
//? noassist add_import org
