import java.util.List;
import java.util./*@bad_import*/Lisst;

class Main {
    int count;

    void run(List<String> names) {
        int a = /*@name*/missing + 1;
        names./*@method*/frobnicate();
        int b = this./*@field*/nothing;
        String s = names.get(0)./*@chain*/nope();
        /*@call*/helper(1);
        count = a + b + s.length();
    }
}
//? diag unresolved-import bad_import
//? diag unresolved-name name
//? diag unresolved-method method
//? diag unresolved-field field
//? diag unresolved-method chain
//? diag unresolved-method call
//? diag exact
