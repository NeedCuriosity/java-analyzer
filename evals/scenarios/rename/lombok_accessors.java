//- /lombok/Data.java
package lombok;
public @interface Data {}
//- /lombok/Builder.java
package lombok;
public @interface Builder {}
//- /app/RenameDto.java
package app;

import lombok.Builder;
import lombok.Data;

@Data
@Builder
public class RenameDto {
    private String /*@name*/name;
    private boolean /*@active*/active;
    private boolean isAdmin;
}
//- /app/Use.java
package app;

class Use {
    String f(RenameDto d) {
        d./*@set*/setName("x");
        d.setActive(!d./*@is*/isActive());
        d./*@set_admin*/setAdmin(d.isAdmin());
        RenameDto b = RenameDto.builder()./*@bname*/name("y").active(true).build();
        return d./*@get*/getName() + b.getName();
    }
}
//? rename name title ; has "private String title;" ; has "d.setTitle(\"x\");" ; has "return d.getTitle() + b.getTitle();" ; has "builder().title(\"y\")" ; lacks "Name" ; lacks ".name("
//? rename get title ; fails
//? rename get getTitle ; has "private String title;" ; has "d.setTitle(\"x\");" ; has "return d.getTitle() + b.getTitle();" ; has "builder().title(\"y\")" ; lacks "getGetTitle" ; lacks "Name"
//? rename set setTitle ; has "private String title;" ; has "d.setTitle(\"x\");" ; has "d.getTitle()"
//? rename bname title ; has "private String title;" ; has "d.getTitle()"
//? rename active enabled ; has "private boolean enabled;" ; has "d.setEnabled(!d.isEnabled());" ; has ".enabled(true)" ; lacks "Active"
//? rename is isOn ; has "private boolean on;" ; has "d.setOn(!d.isOn());" ; has ".on(true)"
//? rename set_admin setRoot ; has "private boolean isRoot;" ; has "d.setRoot(d.isRoot());" ; lacks "Admin"
