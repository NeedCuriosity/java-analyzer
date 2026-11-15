import java.util.List;
import java.util./*@unused_import*/Map;

class Main {
    private int /*@unused_field*/never;
    private int used;

    private void /*@unused_method*/helper() {}

    int run(List<String> names, String a, String b) {
        int /*@unused_local*/tmp = 1;
        int x = used;
        /*@self_assign*/x = x;
        if (a /*@string_eq*/== b) {
            return 1;
        }
        if (/*@self_cmp*/x == x) {
            return 2;
        }
        return names.size();
    }
}
//? diag unused-import unused_import
//? diag unused-private-member unused_field
//? diag unused-private-member unused_method
//? diag unused-local unused_local
//? diag self-assignment self_assign
//? diag string-equality string_eq
//? diag self-comparison self_cmp
//? diag none
