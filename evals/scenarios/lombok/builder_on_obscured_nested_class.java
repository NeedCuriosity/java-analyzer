//- /lombok/Data.java
package lombok;
public @interface Data {}
//- /lombok/Builder.java
package lombok;
public @interface Builder {}
//- /app/BucketPolicyConfigDto.java
package app;

import java.util.List;
import lombok.Builder;
import lombok.Data;

@Data
@Builder
public class BucketPolicyConfigDto {
    private String Version;
    private List<Statement> Statement;

    @Data
    @Builder
    public static class Statement {
        private String Effect;
        private String Principal;
        private String Action;
        private String Resource;
    }
}
//- /app/MinioController.java
package app;

import java.util.List;

class MinioController {
    BucketPolicyConfigDto policy(String bucketName) {
        BucketPolicyConfigDto.Statement statement = BucketPolicyConfigDto.Statement./*@builder*/builder()
                .Effect("Allow")
                .Principal("*")
                .Action("s3:GetObject")
                .Resource("arn:aws:s3:::" + bucketName + "/*.**").build();
        return BucketPolicyConfigDto.builder()
                .Version("2012-10-17")
                .Statement(List.of(statement))
                .build();
    }
}
//? diag none
//? hover builder ~ "builder()"
