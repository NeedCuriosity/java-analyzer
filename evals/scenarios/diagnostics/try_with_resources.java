import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;

class Main {
    int read(String path) throws IOException {
        InputStream in = new FileInputStream(path);
        /*@t1*/try {
            return in.read();
        } finally {
            in.close();
        }
    }

    int nullChecked(String path) throws IOException {
        FileInputStream in = new FileInputStream(path);
        /*@t2*/try {
            return in.read();
        } finally {
            if (in != null) {
                in.close();
            }
        }
    }

    int usedAfter(String path) throws IOException {
        InputStream in = new FileInputStream(path);
        try {
            in.read();
        } finally {
            in.close();
        }
        return in.available();
    }

    int withCatch(String path) throws IOException {
        InputStream in = new FileInputStream(path);
        try {
            return in.read();
        } catch (RuntimeException e) {
            return -1;
        } finally {
            in.close();
        }
    }

    void moreInFinally(String path) throws IOException {
        InputStream in = new FileInputStream(path);
        try {
            in.read();
        } finally {
            System.out.println("done");
            in.close();
        }
    }

    void notCloseable(StringBuilder sb) {
        Holder h = new Holder();
        try {
            sb.append(h);
        } finally {
            h.close();
        }
    }

    static class Holder {
        void close() {}
    }
}
//? diag try-with-resources t1
//? diag try-with-resources t2
//? diag only try-with-resources
//? assist fix_redundancy t1 ; has "    int read(String path) throws IOException {\n        try (InputStream in = new FileInputStream(path)) {\n            return in.read();\n        }\n    }"
//? assist fix_redundancy t2 ; has "try (FileInputStream in = new FileInputStream(path)) {\n            return in.read();\n        }\n    }"
