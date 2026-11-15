import java.util.List;

class Main {
    enum Color { RED, GREEN }

    void run(String name, int count, List<String> names, Color color) {
        String alias = na/*@string_ctx*/;
        int total = co/*@int_ctx*/;
        switch (color) {
            case /*@case_ctx*/
        }
        Color picked = Color./*@enum_ctx*/;
        take(/*@arg_ctx*/);
    }

    void take(List<String> items) {}
}
//? complete string_ctx first name
//? complete string_ctx has names
//? complete int_ctx first count
//? complete case_ctx has RED GREEN
//? complete enum_ctx has RED GREEN values valueOf
//? complete arg_ctx first names
