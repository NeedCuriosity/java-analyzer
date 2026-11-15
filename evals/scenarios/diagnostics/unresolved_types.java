import java.util.List;

class Main {
    /*@field_ty*/Widget widget;

    /*@ret_ty*/Gadget make(/*@param_ty*/Gizmo g) { return null; }

    void run() {
        /*@local_ty*/Thing t = null;
        List</*@type_arg*/Stuff> xs = null;
        Object o = new /*@new_ty*/Missing();
    }
}
//? diag unresolved-type field_ty
//? diag unresolved-type ret_ty
//? diag unresolved-type param_ty
//? diag unresolved-type local_ty
//? diag unresolved-type type_arg
//? diag unresolved-type new_ty
//? diag exact
